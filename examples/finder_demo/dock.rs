use std::path::{Path, PathBuf};
use std::process::Command;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDictionary, NSNumber, NSString, NSURL,
    NSURLBookmarkCreationOptions, NSUserDefaults,
};

const DOCK: &str = "com.apple.dock";
const APPS: &str = "persistent-apps";
const OTHERS: &str = "persistent-others";

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Text(String),
    Number(i64),
    Data(Vec<u8>),
    Dict(Vec<(&'static str, Value)>),
}

#[derive(Debug, PartialEq)]
struct Tile {
    shelf: &'static str,
    address: String,
    entry: Value,
}

fn tile(path: &Path) -> Option<Tile> {
    let folder = path.is_dir();
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path.to_str()?), folder);
    let address = url.absoluteString()?.to_string();
    let app = folder && path.extension().is_some_and(|ext| ext == "app");
    let label = match app {
        true => path.file_stem(),
        false => path.file_name(),
    }?
    .to_string_lossy()
    .into_owned();
    let mut data = vec![
        (
            "file-data",
            Value::Dict(vec![
                ("_CFURLString", Value::Text(address.clone())),
                ("_CFURLStringType", Value::Number(15)),
            ]),
        ),
        ("file-label", Value::Text(label)),
    ];
    if let Some(book) = url
        .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::empty(),
            None,
            None,
        )
        .ok()
    {
        data.push(("book", Value::Data(book.to_vec())));
    }
    let (shelf, kind) = match (app, folder) {
        (true, _) => {
            data.push(("file-type", Value::Number(41)));
            let id = NSBundle::bundleWithPath(&NSString::from_str(path.to_str()?))
                .and_then(|bundle| bundle.bundleIdentifier());
            if let Some(id) = id {
                data.push(("bundle-identifier", Value::Text(id.to_string())));
            }
            (APPS, "file-tile")
        }
        (false, true) => {
            data.push(("file-type", Value::Number(2)));
            (OTHERS, "directory-tile")
        }
        (false, false) => (OTHERS, "file-tile"),
    };
    Some(Tile {
        shelf,
        address,
        entry: Value::Dict(vec![
            ("tile-data", Value::Dict(data)),
            ("tile-type", Value::Text(kind.to_owned())),
        ]),
    })
}

fn object(value: &Value) -> Retained<AnyObject> {
    match value {
        Value::Text(text) => NSString::from_str(text).into(),
        Value::Number(number) => NSNumber::new_i64(*number).into(),
        Value::Data(bytes) => NSData::with_bytes(bytes).into(),
        Value::Dict(pairs) => {
            let keys: Vec<Retained<NSString>> = pairs
                .iter()
                .map(|(key, _)| NSString::from_str(key))
                .collect();
            let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
            let values: Vec<Retained<AnyObject>> =
                pairs.iter().map(|(_, value)| object(value)).collect();
            NSDictionary::<NSString, AnyObject>::from_retained_objects(&keys, &values).into()
        }
    }
}

fn address_of(kept: &AnyObject) -> Option<String> {
    let lookup = |dict: &AnyObject, key: &str| {
        dict.downcast_ref::<NSDictionary>()?
            .objectForKey(&NSString::from_str(key))
    };
    let data = lookup(kept, "tile-data")?;
    let file = lookup(&data, "file-data")?;
    Some(
        lookup(&file, "_CFURLString")?
            .downcast::<NSString>()
            .ok()?
            .to_string(),
    )
}

pub(crate) fn add(paths: &[PathBuf]) -> Result<(), String> {
    let defaults =
        NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&NSString::from_str(DOCK)))
            .ok_or("The Dock’s settings can’t be read.")?;
    let mut added = false;
    for tile in paths.iter().filter_map(|path| tile(path)) {
        let key = NSString::from_str(tile.shelf);
        let mut shelf: Vec<Retained<AnyObject>> = defaults
            .arrayForKey(&key)
            .map(|kept| kept.to_vec())
            .unwrap_or_default();
        if shelf
            .iter()
            .any(|kept| address_of(kept).as_deref() == Some(tile.address.as_str()))
        {
            continue;
        }
        shelf.push(object(&tile.entry));
        unsafe { defaults.setObject_forKey(Some(&NSArray::from_retained_slice(&shelf)), &key) };
        added = true;
    }
    if !added {
        return Ok(());
    }
    defaults.synchronize();
    // The Dock reads its lists only as it starts.
    Command::new("/usr/bin/killall")
        .arg("Dock")
        .status()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(tile: &Tile) -> (&str, &str, Vec<(&'static str, Value)>) {
        let Value::Dict(entry) = &tile.entry else {
            panic!("a tile is a dictionary");
        };
        let kind = entry
            .iter()
            .find_map(|(key, value)| match (*key, value) {
                ("tile-type", Value::Text(kind)) => Some(kind.as_str()),
                _ => None,
            })
            .expect("a tile names its type");
        let data = entry
            .iter()
            .find_map(|(key, value)| match (*key, value) {
                ("tile-data", Value::Dict(data)) => Some(data.clone()),
                _ => None,
            })
            .expect("a tile carries its data");
        let data = data.into_iter().filter(|(key, _)| *key != "book").collect();
        (tile.shelf, kind, data)
    }

    fn file_data(address: &str) -> (&'static str, Value) {
        (
            "file-data",
            Value::Dict(vec![
                ("_CFURLString", Value::Text(address.to_owned())),
                ("_CFURLStringType", Value::Number(15)),
            ]),
        )
    }

    #[test]
    fn an_app_joins_the_apps_under_its_bundle_id() {
        let root = std::env::temp_dir().join(format!("ngx-dock-app-{}", std::process::id()));
        let app = root.join("Sample.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>dev.example.sample</string></dict></plist>",
        )
        .unwrap();
        let tile = tile(&app).unwrap();
        let address = format!("file://{}/", app.display());
        assert_eq!(tile.address, address);
        assert_eq!(
            fields(&tile),
            (
                APPS,
                "file-tile",
                vec![
                    file_data(&address),
                    ("file-label", Value::Text("Sample".into())),
                    ("file-type", Value::Number(41)),
                    (
                        "bundle-identifier",
                        Value::Text("dev.example.sample".into())
                    ),
                ]
            )
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_folder_is_a_stack_and_a_file_a_tile_at_the_far_end() {
        let root = std::env::temp_dir().join(format!("ngx-dock-doc-{}", std::process::id()));
        let file = root.join("notes.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&file, "x").unwrap();
        let folder = tile(&root).unwrap();
        let address = format!("file://{}/", root.display());
        let label = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            fields(&folder),
            (
                OTHERS,
                "directory-tile",
                vec![
                    file_data(&address),
                    ("file-label", Value::Text(label)),
                    ("file-type", Value::Number(2)),
                ]
            )
        );
        let document = tile(&file).unwrap();
        assert_eq!(
            fields(&document),
            (
                OTHERS,
                "file-tile",
                vec![
                    file_data(&format!("file://{}", file.display())),
                    ("file-label", Value::Text("notes.txt".into())),
                ]
            )
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
