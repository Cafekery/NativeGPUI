use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use finder_fs::ds_store::{Background, IconOptions};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSJSONWritingOptions,
    NSKeyedUnarchiver, NSMutableDictionary, NSNumber, NSString, NSURL, NSUserDefaults,
};

use crate::chrome::scope_bar::{THIS_FOLDER, THIS_MAC};
use crate::data::Column;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW, ICON_VIEW, LIST_VIEW};

const VIEW: &str = "ng.finder.view";
const SIDEBAR: &str = "sidebar.json";
const SCOPE: &str = "ng.finder.search.scope";
const PANE: &str = "ng.finder.pane";
const GALLERY_PANE: &str = "ng.finder.pane.gallery";

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

pub(crate) struct Finder {
    pub(crate) posix_title: bool,
    pub(crate) text_size: f32,
    pub(crate) icon_size: f32,
    pub(crate) relative_dates: bool,
    pub(crate) calculate_all_sizes: bool,
    pub(crate) toolbar: Vec<String>,
    pub(crate) sort: (Column, bool),
    pub(crate) pane_widths: (Option<f32>, Option<f32>),
    pub(crate) sidebar_width: Option<f32>,
    pub(crate) columns: Vec<Column>,
    pub(crate) arranged: (Column, Column),
    pub(crate) icon_options: IconOptions,
    pub(crate) group: Column,
}

fn column_view_key(code: &str) -> Option<Column> {
    Some(match code {
        "dnam" => Column::Name,
        "kipl" => Column::Kind,
        "ludt" => Column::Opened,
        "pAdd" => Column::Added,
        "modd" => Column::Modified,
        "ascd" => Column::Created,
        "logs" => Column::Size,
        "ftat" => Column::Tags,
        _ => return None,
    })
}

fn group_key(name: &str) -> Option<Column> {
    Some(match name {
        "Name" => Column::Name,
        "Kind" => Column::Kind,
        "Application" => Column::Application,
        "Date Last Opened" => Column::Opened,
        "Date Added" => Column::Added,
        "Date Modified" => Column::Modified,
        "Date Created" => Column::Created,
        "Size" => Column::Size,
        "Tags" => Column::Tags,
        _ => return None,
    })
}

fn value(of: &NSDictionary, key: &str) -> Option<Retained<AnyObject>> {
    of.objectForKey(&NSString::from_str(key))
}

static FINDER: LazyLock<Finder> = LazyLock::new(|| {
    let defaults = finder_domain();
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
    let icons = dict("StandardViewSettings").and_then(|all| {
        value(&all, "IconViewSettings")?
            .downcast::<NSDictionary>()
            .ok()
    });
    let sort_id = list.as_deref().and_then(|list| text(list, "sortColumn"));
    let listed: Vec<Retained<NSDictionary>> = list
        .as_deref()
        .and_then(|list| value(list, "columns")?.downcast::<NSArray>().ok())
        .map_or_else(Vec::new, |columns| {
            columns
                .iter()
                .filter_map(|column| column.downcast::<NSDictionary>().ok())
                .collect()
        });
    let flagged = |column: &NSDictionary, key: &str| {
        value(column, key)
            .and_then(|on| on.downcast::<NSNumber>().ok())
            .map(|on| on.boolValue())
    };
    let ascending = sort_id.as_deref().and_then(|id| {
        listed.iter().find_map(|column| {
            (text(column, "identifier")? == id).then_some(())?;
            flagged(column, "ascending")
        })
    });
    let sort = sort_id.as_deref().and_then(Column::from_id);
    let columns: Vec<Column> = listed
        .iter()
        .filter(|column| flagged(column, "visible") == Some(true))
        .filter_map(|column| Column::from_id(&text(column, "identifier")?))
        .filter(|column| *column != Column::Name)
        .collect();
    let arranged = |settings: &str, view: &str, key: &str| -> Option<String> {
        let all = dict(settings)?;
        let view = value(&all, view)?.downcast::<NSDictionary>().ok()?;
        text(&view, key)
    };
    let column_view = arranged("StandardViewOptions", "ColumnViewOptions", "ArrangeBy")
        .and_then(|code| column_view_key(&code));
    let gallery_view = arranged("StandardViewSettings", "GalleryViewSettings", "arrangeBy")
        .and_then(|id| Column::from_id(&id));
    let flag = |key: &str| {
        read(key)
            .and_then(|on| on.downcast::<NSNumber>().ok())
            .is_some_and(|on| on.boolValue())
    };
    let width = |key: &str| {
        let found = read(key)?.downcast::<NSNumber>().ok()?;
        Some(found.doubleValue() as f32)
    };
    Finder {
        posix_title: flag("_FXShowPosixPathInTitle"),
        text_size: number("textSize").unwrap_or(13.) as f32,
        icon_size: number("iconSize").unwrap_or(16.) as f32,
        relative_dates: number("useRelativeDates").is_none_or(|on| on != 0.),
        calculate_all_sizes: number("calculateAllSizes").is_some_and(|on| on != 0.),
        toolbar: toolbar.map_or_else(Vec::new, |ids| {
            ids.iter()
                .filter_map(|id| Some(id.downcast_ref::<NSString>()?.to_string()))
                .collect()
        }),
        sort: sort.map_or((Column::Name, true), |column| {
            (column, ascending.unwrap_or(true))
        }),
        pane_widths: (width("PreviewPaneWidth"), width("PreviewPaneGalleryWidth")),
        sidebar_width: width("SidebarWidth2"),
        columns: match listed.is_empty() {
            true => vec![Column::Modified, Column::Size, Column::Kind],
            false => columns,
        },
        arranged: (
            column_view.unwrap_or(Column::Name),
            gallery_view.unwrap_or(Column::Name),
        ),
        icon_options: icons.map_or_else(IconOptions::default, |icons| IconOptions::read(&icons)),
        group: finder_string("FXPreferredGroupBy")
            .and_then(|name| group_key(&name))
            .unwrap_or(Column::Kind),
    }
});

pub(crate) fn finder() -> &'static Finder {
    &FINDER
}

fn finder_domain() -> Option<Retained<NSUserDefaults>> {
    let suite = NSString::from_str("com.apple.finder");
    NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&suite))
}

fn setting(key: &str) -> Option<Retained<AnyObject>> {
    let key = NSString::from_str(key);
    defaults()
        .objectForKey(&key)
        .or_else(|| finder_domain()?.objectForKey(&key))
}

pub(crate) fn setting_on(key: &str, default: bool) -> bool {
    let Some(value) = setting(key) else {
        return default;
    };
    if let Some(number) = value.downcast_ref::<NSNumber>() {
        return number.boolValue();
    }
    value.downcast_ref::<NSString>().map_or(default, |text| {
        matches!(
            text.to_string().trim().chars().next(),
            Some('Y' | 'y' | 'T' | 't' | '1'..='9')
        )
    })
}

pub(crate) fn set_setting_on(key: &str, on: bool) {
    defaults().setBool_forKey(on, &NSString::from_str(key));
}

pub(crate) fn setting_text(key: &str) -> Option<String> {
    Some(setting(key)?.downcast::<NSString>().ok()?.to_string())
}

pub(crate) fn set_setting_text(key: &str, value: &str) {
    let value = NSString::from_str(value);
    unsafe { defaults().setObject_forKey(Some(&value), &NSString::from_str(key)) };
}

pub(crate) fn opens_folders_in_tabs() -> bool {
    setting_on("FinderSpawnTab", true)
}

pub(crate) fn folders_first() -> bool {
    setting_on("_FXSortFoldersFirst", false)
}

pub(crate) fn shows_all_extensions() -> bool {
    setting_on("AppleShowAllExtensions", false)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WindowTarget {
    Recents,
    Computer,
    Folder(PathBuf),
}

pub(crate) fn new_window_target() -> WindowTarget {
    use finder_fs::volumes::{Standard, standard};
    let home = standard(Standard::Home).unwrap_or_else(|| PathBuf::from("/"));
    let stored = || {
        let url = NSURL::URLWithString(&NSString::from_str(&setting_text("NewWindowTargetPath")?))?;
        Some(PathBuf::from(url.path()?.to_string()))
    };
    let folder = match setting_text("NewWindowTarget").as_deref() {
        Some("PfCm") => return WindowTarget::Computer,
        Some("PfVo") => stored().unwrap_or_else(|| PathBuf::from("/")),
        Some("PfHm") => home.clone(),
        Some("PfDe") => home.join("Desktop"),
        Some("PfDo") => home.join("Documents"),
        Some("PfID") => finder_fs::icloud::drive().unwrap_or_else(|| home.clone()),
        Some("PfLo") => stored().unwrap_or_else(|| home.clone()),
        _ => return WindowTarget::Recents,
    };
    WindowTarget::Folder(if folder.is_dir() { folder } else { home })
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

fn sidebar_path() -> Retained<NSString> {
    NSString::from_str(&crate::store().join(SIDEBAR).to_string_lossy())
}

fn sidebar_file() -> HashMap<String, Vec<String>> {
    let Some(map) = NSData::dataWithContentsOfFile(&sidebar_path())
        .and_then(|data| {
            NSJSONSerialization::JSONObjectWithData_options_error(
                &data,
                NSJSONReadingOptions::empty(),
            )
            .ok()
        })
        .and_then(|object| object.downcast::<NSDictionary>().ok())
    else {
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
    let (keys, lists): (Vec<_>, Vec<_>) = kept
        .iter()
        .map(|(key, values)| {
            let strings: Vec<_> = values
                .iter()
                .map(|value| NSString::from_str(value))
                .collect();
            (
                NSString::from_str(key),
                NSArray::from_retained_slice(&strings),
            )
        })
        .unzip();
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let lists: Vec<&NSArray<NSString>> = lists.iter().map(|list| &**list).collect();
    let map = NSDictionary::from_slices(&keys, &lists);
    let written = unsafe {
        NSJSONSerialization::dataWithJSONObject_options_error(
            &map,
            NSJSONWritingOptions::PrettyPrinted,
        )
    };
    if let Ok(data) = written {
        data.writeToFile_atomically(&sidebar_path(), true);
    }
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

fn finder_pref(key: &str) -> Option<Retained<AnyObject>> {
    finder_domain()?.objectForKey(&NSString::from_str(key))
}

fn finder_string(key: &str) -> Option<String> {
    Some(finder_pref(key)?.downcast::<NSString>().ok()?.to_string())
}

fn finder_strings(key: &str) -> Vec<String> {
    let Some(list) = finder_pref(key).and_then(|list| list.downcast::<NSArray>().ok()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|value| Some(value.downcast_ref::<NSString>()?.to_string()))
        .collect()
}

pub(crate) fn go_history() -> Vec<String> {
    sidebar_file()
        .remove("go.history")
        .unwrap_or_else(|| finder_strings("GoToFieldHistory"))
}

pub(crate) fn go_field() -> Option<String> {
    match sidebar_file().remove("go.history") {
        Some(own) => own.into_iter().next(),
        None => finder_string("GoToField"),
    }
}

pub(crate) const GO_SHEET: &str = "GoToSheet";

pub(crate) fn go_sheet_size() -> Option<(f32, f32)> {
    let key = format!("NSWindow Frame {GO_SHEET}");
    let frame = read_text(&key).or_else(|| finder_string(&key))?;
    let mut sides = frame
        .split_whitespace()
        .skip(2)
        .map(|side| side.parse::<f32>().ok());
    Some((sides.next()??, sides.next()??))
}

pub(crate) fn set_go_history(paths: &[&str]) {
    write_list("go.history", paths);
}

const CONNECT_LAST: &str = "ng.finder.connect.last";
pub(crate) const CONNECT_WINDOW: &str = "ConnectToServer";

pub(crate) fn connect_last() -> Option<String> {
    read_text(CONNECT_LAST).or_else(|| finder_string("FXConnectToLastURL"))
}

pub(crate) fn set_connect_last(address: &str) {
    set_setting_text(CONNECT_LAST, address);
}

/// Finder's last Connect to Server frame, in AppKit's bottom-left screen points.
pub(crate) fn finder_connect_frame() -> Option<[f64; 4]> {
    let bounds = finder_string("FXConnectToBounds")?;
    let mut sides = bounds
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|side| !side.is_empty())
        .map(|side| side.parse::<f64>().ok());
    Some([
        sides.next()??,
        sides.next()??,
        sides.next()??,
        sides.next()??,
    ])
}

fn finder_servers(list: &str) -> Vec<String> {
    let file = crate::home().join(format!(
        "Library/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.{list}.sfl4"
    ));
    let Some(data) = NSData::dataWithContentsOfFile(&NSString::from_str(&file.to_string_lossy()))
    else {
        return Vec::new();
    };
    let Ok(archive) = (unsafe {
        NSKeyedUnarchiver::initForReadingFromData_error(NSKeyedUnarchiver::alloc(), &data)
    }) else {
        return Vec::new();
    };
    // Plain dictionaries, arrays, strings and data: nothing a secure decode would refuse to trust.
    let root = unsafe {
        archive.setRequiresSecureCoding(false);
        let root = archive.decodeObjectForKey(&NSString::from_str("root"));
        archive.finishDecoding();
        root
    };
    let items = root
        .and_then(|root| root.downcast::<NSDictionary>().ok())
        .and_then(|root| value(&root, "items")?.downcast::<NSArray>().ok());
    let Some(items) = items else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let item = item.downcast::<NSDictionary>().ok()?;
            Some(
                value(&item, "Name")?
                    .downcast::<NSString>()
                    .ok()?
                    .to_string(),
            )
        })
        .collect()
}

pub(crate) fn favorite_servers() -> Vec<String> {
    sidebar_file()
        .remove("servers.favorite")
        .unwrap_or_else(|| finder_servers("FavoriteServers"))
}

pub(crate) fn set_favorite_servers(servers: &[&str]) {
    write_list("servers.favorite", servers);
}

pub(crate) fn recent_servers() -> Vec<String> {
    sidebar_file()
        .remove("servers.recent")
        .unwrap_or_else(|| finder_servers("RecentServers"))
}

pub(crate) fn set_recent_servers(servers: &[&str]) {
    write_list("servers.recent", servers);
}

pub(crate) fn view() -> Option<usize> {
    read(VIEW).map(|v| v as usize)
}

pub(crate) fn set_view(view: usize) {
    write(VIEW, view as f64);
}

pub(crate) fn opens_in(folder: &str) -> Option<(usize, bool)> {
    let view = read(&format!("ng.finder.opens.{folder}"))?;
    let browses = read(&format!("ng.finder.browses.{folder}")).is_some_and(|on| on != 0.);
    Some((view as usize, browses))
}

pub(crate) fn set_opens_in(folder: &str, opens: Option<(usize, bool)>) {
    let keys = [
        format!("ng.finder.opens.{folder}"),
        format!("ng.finder.browses.{folder}"),
    ];
    match opens {
        Some((view, browses)) => {
            write(&keys[0], view as f64);
            write(&keys[1], f64::from(u8::from(browses)));
        }
        None => {
            for key in keys {
                defaults().removeObjectForKey(&NSString::from_str(&key));
            }
        }
    }
}

pub(crate) fn search_scope() -> usize {
    match setting_text("FXDefaultSearchScope").as_deref() {
        Some("SCcf") => THIS_FOLDER,
        Some("SCsp") => read(SCOPE).map_or_else(
            || match setting_text("FXLastSearchScope").as_deref() {
                Some("SCcf") => THIS_FOLDER,
                _ => THIS_MAC,
            },
            |scope| scope as usize,
        ),
        _ => THIS_MAC,
    }
}

pub(crate) fn set_search_scope(scope: usize) {
    write(SCOPE, scope as f64);
}

pub(crate) fn pane_width(gallery: bool) -> Option<f32> {
    let (key, finder) = match gallery {
        true => (GALLERY_PANE, finder().pane_widths.1),
        false => (PANE, finder().pane_widths.0),
    };
    read(key).map(|v| v as f32).or(finder)
}

pub(crate) fn set_pane_width(gallery: bool, width: f32) {
    write(if gallery { GALLERY_PANE } else { PANE }, width as f64);
}

pub(crate) fn preview_options(kind: &str) -> Option<Vec<String>> {
    let key = NSString::from_str(&format!("ng.finder.preview.options.{kind}"));
    let list = defaults().stringArrayForKey(&key)?;
    Some(list.iter().map(|on| on.to_string()).collect())
}

pub(crate) fn set_preview_options(kind: &str, on: &[&str]) {
    let key = NSString::from_str(&format!("ng.finder.preview.options.{kind}"));
    let strings: Vec<_> = on.iter().map(|on| NSString::from_str(on)).collect();
    let list = NSArray::from_retained_slice(&strings);
    let value: &AnyObject = &list;
    unsafe { defaults().setObject_forKey(Some(value), &key) };
}

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

fn column_key(column: Column) -> &'static str {
    match column {
        Column::Name => "ng.finder.col.name",
        Column::Kind => "ng.finder.col.kind",
        Column::Opened => "ng.finder.col.opened",
        Column::Added => "ng.finder.col.added",
        Column::Modified => "ng.finder.col.modified",
        Column::Created => "ng.finder.col.created",
        Column::Size => "ng.finder.col.size",
        Column::Tags => "ng.finder.col.tags",
        Column::Version => "ng.finder.col.version",
        Column::Comments => "ng.finder.col.comments",
        Column::Application => "ng.finder.col.application",
        Column::SharedBy => "ng.finder.col.shared_by",
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

fn read_text(key: &str) -> Option<String> {
    Some(
        defaults()
            .stringForKey(&NSString::from_str(key))?
            .to_string(),
    )
}

fn spots_key(folder: &str) -> Retained<NSString> {
    NSString::from_str(&format!("ng.finder.icons.at.{folder}"))
}

fn look_key(view: usize, folder: Option<&str>) -> Retained<NSString> {
    let name = match view {
        ICON_VIEW => "icons",
        LIST_VIEW => "list",
        COLUMN_VIEW => "columns",
        _ => "gallery",
    };
    NSString::from_str(&match folder {
        Some(folder) => format!("ng.finder.{name}.view.{folder}"),
        None => format!("ng.finder.{name}.view"),
    })
}

fn saved_look(
    view: usize,
    folder: Option<&str>,
) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    defaults().dictionaryForKey(&look_key(view, folder))
}

fn edit_look(
    view: usize,
    folder: Option<&str>,
    edit: impl FnOnce(&NSMutableDictionary<NSString, AnyObject>),
) {
    let look = NSMutableDictionary::new();
    if let Some(saved) = saved_look(view, folder) {
        look.addEntriesFromDictionary(&saved);
    }
    edit(&look);
    let look: &AnyObject = &look;
    unsafe { defaults().setObject_forKey(Some(look), &look_key(view, folder)) };
}

fn look_value(view: usize, folder: &str, key: &str) -> Option<Retained<AnyObject>> {
    let key = NSString::from_str(key);
    [Some(folder), None]
        .into_iter()
        .find_map(|at| saved_look(view, at)?.objectForKey(&key))
}

pub(crate) fn look_number(view: usize, folder: &str, key: &str) -> Option<f64> {
    let number = look_value(view, folder, key)?;
    Some(number.downcast_ref::<NSNumber>()?.doubleValue())
}

pub(crate) fn look_on(view: usize, folder: &str, key: &str) -> Option<bool> {
    look_number(view, folder, key).map(|on| on != 0.)
}

pub(crate) fn look_text(view: usize, folder: &str, key: &str) -> Option<String> {
    let text = look_value(view, folder, key)?;
    Some(text.downcast_ref::<NSString>()?.to_string())
}

fn set_look(view: usize, folder: &str, key: &str, value: &AnyObject) {
    edit_look(view, Some(folder), |look| {
        look.insert(&*NSString::from_str(key), value)
    });
}

pub(crate) fn set_look_number(view: usize, folder: &str, key: &str, value: f64) {
    set_look(view, folder, key, &NSNumber::new_f64(value));
}

pub(crate) fn set_look_on(view: usize, folder: &str, key: &str, on: bool) {
    set_look(view, folder, key, &NSNumber::new_bool(on));
}

pub(crate) fn set_look_text(view: usize, folder: &str, key: &str, text: &str) {
    set_look(view, folder, key, &NSString::from_str(text));
}

fn columns_key(folder: Option<&str>) -> String {
    match folder {
        Some(folder) => format!("ng.finder.cols.{folder}"),
        None => "ng.finder.cols".into(),
    }
}

pub(crate) fn has_own_look(view: usize, folder: &str) -> bool {
    saved_look(view, Some(folder)).is_some_and(|own| own.count() > 0)
        || view == LIST_VIEW && read_text(&columns_key(Some(folder))).is_some()
}

pub(crate) fn use_as_defaults(view: usize, folder: &str) {
    let own = saved_look(view, Some(folder));
    edit_look(view, None, |kept| {
        if let Some(own) = own {
            kept.addEntriesFromDictionary(&own);
        }
    });
    defaults().removeObjectForKey(&look_key(view, Some(folder)));
    if view != LIST_VIEW {
        return;
    }
    if let Some(ids) = read_text(&columns_key(Some(folder))) {
        set_setting_text(&columns_key(None), &ids);
        defaults().removeObjectForKey(&NSString::from_str(&columns_key(Some(folder))));
    }
}

fn read_icon_options(folder: Option<&str>) -> IconOptions {
    defaults()
        .objectForKey(&look_key(ICON_VIEW, folder))
        .and_then(|options| options.downcast::<NSDictionary>().ok())
        .map_or_else(IconOptions::default, |options| IconOptions::read(&options))
}

pub(crate) fn icon_options(folder: &str) -> IconOptions {
    read_icon_options(Some(folder))
}

pub(crate) fn icon_defaults() -> IconOptions {
    read_icon_options(None)
}

pub(crate) fn list_columns(folder: &str) -> Option<Vec<Column>> {
    let ids = read_text(&columns_key(Some(folder))).or_else(|| read_text(&columns_key(None)))?;
    Some(ids.split(',').filter_map(Column::from_id).collect())
}

pub(crate) fn set_list_columns(folder: &str, columns: &[Column]) {
    let ids: Vec<&str> = columns.iter().map(|column| column.id()).collect();
    set_setting_text(&columns_key(Some(folder)), &ids.join(","));
}

pub(crate) fn arranged(view: usize) -> Option<Column> {
    Column::from_id(&read_text(&format!("ng.finder.arranged.{view}"))?)
}

pub(crate) fn set_arranged(view: usize, column: Column) {
    set_setting_text(&format!("ng.finder.arranged.{view}"), column.id());
}

pub(crate) fn set_icon_options(folder: &str, options: &IconOptions) {
    edit_look(ICON_VIEW, Some(folder), |look| {
        let put = |key: &str, value: &AnyObject| look.insert(&*NSString::from_str(key), value);
        let number = |key, value: f32| put(key, &NSNumber::new_f64(f64::from(value)));
        if let Some(arrange) = &options.arrange_by {
            put("arrangeBy", &NSString::from_str(arrange));
        }
        let numbers = [
            ("iconSize", options.icon_size),
            ("gridSpacing", options.grid_spacing),
            ("textSize", options.text_size),
        ];
        for (key, value) in numbers {
            if let Some(value) = value {
                number(key, value);
            }
        }
        let flags = [
            ("labelOnBottom", options.label_on_bottom),
            ("showIconPreview", options.show_icon_preview),
            ("showItemInfo", options.show_item_info),
        ];
        for (key, on) in flags {
            if let Some(on) = on {
                put(key, &NSNumber::new_bool(on));
            }
        }
        match &options.background {
            Some(Background::Default) => number("backgroundType", 0.),
            Some(Background::Color(parts)) => {
                number("backgroundType", 1.);
                let keys = [
                    "backgroundColorRed",
                    "backgroundColorGreen",
                    "backgroundColorBlue",
                ];
                for (key, part) in keys.into_iter().zip(parts) {
                    number(key, *part);
                }
            }
            Some(Background::Picture(path)) => {
                number("backgroundType", 2.);
                put("backgroundImagePath", &NSString::from_str(path));
            }
            None => {}
        }
    });
}

pub(crate) fn icon_spots(folder: &str) -> HashMap<String, (f32, f32)> {
    let Some(spots) = defaults().dictionaryForKey(&spots_key(folder)) else {
        return HashMap::new();
    };
    spots
        .keys()
        .filter_map(|name| {
            let at = spots.objectForKey(&name)?.downcast::<NSArray>().ok()?;
            let axis = |index| {
                Some(
                    at.objectAtIndex(index)
                        .downcast::<NSNumber>()
                        .ok()?
                        .floatValue(),
                )
            };
            (at.count() == 2).then_some(())?;
            Some((name.to_string(), (axis(0)?, axis(1)?)))
        })
        .collect()
}

pub(crate) fn set_icon_spots<'a>(folder: &str, spots: impl Iterator<Item = (&'a str, (f32, f32))>) {
    let (names, places): (Vec<_>, Vec<_>) = spots
        .map(|(name, (x, y))| {
            let place = NSArray::from_retained_slice(&[NSNumber::new_f32(x), NSNumber::new_f32(y)]);
            (NSString::from_str(name), place)
        })
        .unzip();
    let names: Vec<&NSString> = names.iter().map(|name| &**name).collect();
    let places: Vec<&NSArray<NSNumber>> = places.iter().map(|place| &**place).collect();
    let value: &AnyObject = &NSDictionary::from_slices(&names, &places);
    unsafe { defaults().setObject_forKey(Some(value), &spots_key(folder)) };
}

fn finder_number(group: &str, view: &str, key: &str) -> Option<f64> {
    let all = finder_pref(group)?.downcast::<NSDictionary>().ok()?;
    let view = value(&all, view)?.downcast::<NSDictionary>().ok()?;
    let number = value(&view, key)?;
    Some(number.downcast_ref::<NSNumber>()?.doubleValue())
}

pub(crate) fn view_option(options: &str, key: &str) -> Option<f64> {
    read(&format!("ng.finder.{options}.{key}"))
        .or_else(|| finder_number("StandardViewOptions", options, key))
}

pub(crate) fn set_view_option(options: &str, key: &str, value: f64) {
    write(&format!("ng.finder.{options}.{key}"), value);
}

pub(crate) fn finder_look_number(settings: &str, key: &str) -> Option<f64> {
    finder_number("StandardViewSettings", settings, key)
}
