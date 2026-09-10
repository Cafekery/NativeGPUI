use std::path::{Path, PathBuf};

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

pub(crate) struct App {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) default: bool,
}

fn url(path: &Path) -> Option<objc2::rc::Retained<NSURL>> {
    let path = path.to_str()?;
    unsafe { Some(NSURL::fileURLWithPath(&NSString::from_str(path))) }
}

fn name_of(bundle: &Path) -> String {
    bundle
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every app that says it can open the file, the one that would take a double-click first.
pub(crate) fn apps_for(file: &Path) -> Vec<App> {
    let Some(url) = url(file) else {
        return Vec::new();
    };
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let fallback = unsafe { workspace.URLForApplicationToOpenURL(&url) }
        .and_then(|url| unsafe { url.path() })
        .map(|path| PathBuf::from(path.to_string()));
    let mut apps: Vec<App> = unsafe { workspace.URLsForApplicationsToOpenURL(&url) }
        .iter()
        .filter_map(|url| unsafe { url.path() })
        .map(|path| PathBuf::from(path.to_string()))
        .map(|path| App {
            name: name_of(&path),
            default: Some(&path) == fallback.as_ref(),
            path,
        })
        .collect();
    apps.sort_by(|a, b| b.default.cmp(&a.default).then_with(|| a.name.cmp(&b.name)));
    apps
}

pub(crate) fn open_with(files: &[PathBuf], app: &Path) {
    let Some(app) = url(app) else {
        return;
    };
    let urls: Vec<objc2::rc::Retained<NSURL>> = files.iter().filter_map(|file| url(file)).collect();
    if urls.is_empty() {
        return;
    }
    let refs: Vec<&NSURL> = urls.iter().map(|url| url.as_ref()).collect();
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    unsafe {
        workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
            &objc2_foundation::NSArray::from_slice(&refs),
            &app,
            &objc2_app_kit::NSWorkspaceOpenConfiguration::configuration(),
            None,
        )
    };
}
