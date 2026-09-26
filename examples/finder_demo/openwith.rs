use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2_app_kit::{NSWorkspace, NSWorkspaceLaunchOptions};
use objc2_foundation::{NSDictionary, NSString, NSURL, NSURLBookmarkResolutionOptions};

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

/// Each file in the app it is set to open in; LaunchServices asks the user when there is none,
/// as it does for Finder.
pub(crate) fn open_default(files: &[PathBuf]) {
    let workspace = NSWorkspace::sharedWorkspace();
    let configuration = objc2_app_kit::NSWorkspaceOpenConfiguration::configuration();
    for url in files.iter().filter_map(|file| url(file)) {
        workspace.openURL_configuration_completionHandler(&url, &configuration, None);
    }
}

/// The folder a Finder alias points at, which Finder opens in its own window.
pub(crate) fn alias_folder(file: &Path) -> Option<PathBuf> {
    let alias = url(file)?;
    let target = NSURL::URLByResolvingAliasFileAtURL_options_error(
        &alias,
        NSURLBookmarkResolutionOptions::WithoutUI,
    )
    .ok()?;
    let target = PathBuf::from(target.path()?.to_string());
    (target != file && target.is_dir()).then_some(target)
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
