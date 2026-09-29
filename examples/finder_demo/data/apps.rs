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
        .and_then(|app| app.path())
        .map(|app| app.to_string());
    let named =
        (opener.as_ref()).and_then(|opener| APPS.lock().unwrap().names.get(opener).cloned());
    let app = named.or_else(|| {
        let opener = opener?;
        let name = {
            let name = finder_fs::name::of_path(Path::new(&opener));
            SharedString::from(name.strip_suffix(".app").unwrap_or(&name).to_owned())
        };
        APPS.lock().unwrap().names.insert(opener, name.clone());
        Some(name)
    });
    let mut apps = APPS.lock().unwrap();
    if apps.items.len() >= ITEMS_HELD {
        apps.items.clear();
    }
    apps.items.insert(path.clone(), (asked, app.clone()));
    app
}

/// Each row's default app by name, asked of LaunchServices once per path and modification date.
pub(crate) fn read_apps(rows: &mut [Arc<Row>]) {
    for row in rows.iter_mut() {
        let app = app_of(&row.path, row.modified_at);
        if row.app != app {
            *row = Arc::new(Row {
                app,
                ..(**row).clone()
            });
        }
    }
}
