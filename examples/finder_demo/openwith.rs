use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2_app_kit::{NSWorkspace, NSWorkspaceLaunchOptions};
use objc2_foundation::{NSDictionary, NSString, NSURL};

pub(crate) struct App {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) default: bool,
}

fn url(path: &Path) -> Option<Retained<NSURL>> {
    Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
}

fn name_of(bundle: &Path) -> String {
    bundle
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn apps_for(file: &Path) -> Vec<App> {
    let Some(url) = url(file) else {
        return Vec::new();
    };
    let workspace = NSWorkspace::sharedWorkspace();
    let fallback = workspace
        .URLForApplicationToOpenURL(&url)
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));
    let mut apps: Vec<App> = workspace
        .URLsForApplicationsToOpenURL(&url)
        .iter()
        .filter_map(|url| url.path())
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

#[allow(deprecated)] // the open API that replaced this call has no print option
pub(crate) fn print(files: &[PathBuf]) -> Result<(), String> {
    let workspace = NSWorkspace::sharedWorkspace();
    for url in files.iter().filter_map(|file| url(file)) {
        unsafe {
            workspace.openURL_options_configuration_error(
                &url,
                NSWorkspaceLaunchOptions::AndPrint,
                &NSDictionary::new(),
            )
        }
        .map_err(|error| error.localizedDescription().to_string())?;
    }
    Ok(())
}

pub(crate) fn open_with(files: &[PathBuf], app: &Path) {
    let Some(app) = url(app) else {
        return;
    };
    let urls: Vec<Retained<NSURL>> = files.iter().filter_map(|file| url(file)).collect();
    if urls.is_empty() {
        return;
    }
    let refs: Vec<&NSURL> = urls.iter().map(|url| url.as_ref()).collect();
    let workspace = NSWorkspace::sharedWorkspace();
    workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
        &objc2_foundation::NSArray::from_slice(&refs),
        &app,
        &objc2_app_kit::NSWorkspaceOpenConfiguration::configuration(),
        None,
    );
}
