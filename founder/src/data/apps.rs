use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use gpui::SharedString;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use super::row::Row;

const ITEMS_HELD: usize = 16_384;

#[derive(Default)]
struct Apps {
    items: HashMap<SharedString, (u64, Option<SharedString>)>,
    names: HashMap<String, SharedString>,
}

static APPS: LazyLock<Mutex<Apps>> = LazyLock::new(Mutex::default);

fn app_of(path: &SharedString, modified: f64) -> Option<SharedString> {
    let asked = modified.to_bits();
    if let Some((_, app)) = (APPS.lock().unwrap().items.get(path)).filter(|(at, _)| *at == asked) {
        return app.clone();
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let opener = NSWorkspace::sharedWorkspace()
        .URLForApplicationToOpenURL(&url)
        .and_then(|app| app.path());
    let mut apps = APPS.lock().unwrap();
    let app = opener.map(|opener| {
        let entry = apps.names.entry(opener.to_string());
        let name = entry.or_insert_with_key(|opener| {
            let name = finder_fs::name::of_path(Path::new(opener));
            name.strip_suffix(".app").unwrap_or(&name).to_owned().into()
        });
        name.clone()
    });
    if apps.items.len() >= ITEMS_HELD {
        apps.items.clear();
    }
    apps.items.insert(path.clone(), (asked, app.clone()));
    app
}

pub(crate) fn read_apps(rows: &mut [Arc<Row>]) {
    for row in rows
        .iter_mut()
        .filter(|row| !crate::remote::is_remote(&row.path))
    {
        let app = app_of(&row.path, row.modified_at);
        if row.app != app {
            Arc::make_mut(row).app = app;
        }
    }
}
