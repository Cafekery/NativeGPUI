use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use block2::DynBlock;
use gpui::Context;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSControlStateValueOff, NSControlStateValueOn, NSModalResponseOK, NSOpenPanel,
    NSOpenSavePanelDelegate, NSPopUpButton, NSTextField, NSView, NSWorkspace,
    NSWorkspaceLaunchOptions, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSURL, NSURLBookmarkResolutionOptions, NSURLContentTypeKey,
};

use crate::app::FinderDemo;

pub(crate) struct App {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) default: bool,
}

pub(crate) fn url(path: &Path) -> Option<Retained<NSURL>> {
    Some(NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?)))
}

fn paths(urls: impl IntoIterator<Item = Retained<NSURL>>) -> Vec<PathBuf> {
    urls.into_iter()
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

fn version_of(bundle: &Path) -> Option<String> {
    let bundle = NSBundle::bundleWithPath(&NSString::from_str(bundle.to_str()?))?;
    let version =
        bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))?;
    Some(version.downcast::<NSString>().ok()?.to_string())
}

pub(crate) fn apps_for_all(files: &[PathBuf]) -> Vec<App> {
    let workspace = NSWorkspace::sharedWorkspace();
    let mut common: Option<Vec<PathBuf>> = None;
    for file in files {
        let Some(url) = url(file) else {
            return Vec::new();
        };
        let apps = paths(workspace.URLsForApplicationsToOpenURL(&url));
        common = Some(match common {
            Some(kept) => kept.into_iter().filter(|app| apps.contains(app)).collect(),
            None => apps,
        });
    }
    let Some(common) = common else {
        return Vec::new();
    };
    let fallback = files
        .first()
        .and_then(|file| url(file))
        .and_then(|url| workspace.URLForApplicationToOpenURL(&url))
        .and_then(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()));
    let named: Vec<(String, PathBuf)> = common
        .into_iter()
        .map(|path| (finder_fs::name::of_path(&path), path))
        .collect();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (name, _) in &named {
        *seen.entry(name.as_str()).or_default() += 1;
    }
    let twins: Vec<bool> = named
        .iter()
        .map(|(name, _)| seen[name.as_str()] > 1)
        .collect();
    let mut apps: Vec<App> = named
        .into_iter()
        .zip(twins)
        .map(|((name, path), twin)| App {
            name: match twin.then(|| version_of(&path)).flatten() {
                Some(version) => format!("{name} ({version})"),
                None => name,
            },
            default: Some(&path) == fallback.as_ref(),
            path,
        })
        .collect();
    apps.sort_by(|a, b| {
        b.default
            .cmp(&a.default)
            .then_with(|| finder_fs::sort::name_cmp(&a.name, &b.name))
    });
    apps
}

pub(crate) fn apps_for(file: &Path) -> Vec<App> {
    apps_for_all(&[file.to_path_buf()])
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

pub(crate) fn open_default(files: &[PathBuf]) {
    let workspace = NSWorkspace::sharedWorkspace();
    let configuration = NSWorkspaceOpenConfiguration::configuration();
    for url in files.iter().filter_map(|file| url(file)) {
        workspace.openURL_configuration_completionHandler(&url, &configuration, None);
    }
}

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
    NSWorkspace::sharedWorkspace().openURLs_withApplicationAtURL_configuration_completionHandler(
        &NSArray::from_retained_slice(&urls),
        &app,
        &NSWorkspaceOpenConfiguration::configuration(),
        None,
    );
}

pub(crate) fn set_default(files: &[PathBuf], app: &Path) {
    let Some(app) = url(app) else {
        return;
    };
    let workspace = NSWorkspace::sharedWorkspace();
    for file in files.iter().filter_map(|file| url(file)) {
        workspace.setDefaultApplicationAtURL_toOpenFileAtURL_completionHandler(&app, &file, None);
    }
}

/// The app `file` opens with: its own choice where it has one, else its type's.
pub(crate) fn opener(file: &Path) -> Option<PathBuf> {
    let file = url(file)?;
    let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&file)?;
    Some(PathBuf::from(app.path()?.to_string()))
}

fn content_type(file: &Path) -> Option<Retained<AnyObject>> {
    let mut kind = None;
    unsafe { url(file)?.getResourceValue_forKey_error(&mut kind, NSURLContentTypeKey) }.ok()?;
    kind
}

/// The app every document of `file`'s type opens with.
pub(crate) fn type_default(file: &Path) -> Option<PathBuf> {
    let kind = content_type(file)?;
    let app: Option<Retained<NSURL>> = unsafe {
        msg_send![&*NSWorkspace::sharedWorkspace(), URLForApplicationToOpenContentType: &*kind]
    };
    Some(PathBuf::from(app?.path()?.to_string()))
}

pub(crate) fn set_type_default(file: &Path, app: &Path) {
    let (Some(kind), Some(app)) = (content_type(file), url(app)) else {
        return;
    };
    let done: Option<&DynBlock<dyn Fn(*mut NSError)>> = None;
    let _: () = unsafe {
        msg_send![
            &*NSWorkspace::sharedWorkspace(),
            setDefaultApplicationAtURL: &*app,
            toOpenContentType: &*kind,
            completionHandler: done,
        ]
    };
}

pub(crate) fn app_store(files: &[PathBuf]) {
    let store = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&NSString::from_str("com.apple.appstore"))
        .and_then(|url| url.path());
    if let Some(store) = store {
        open_with(&files[..files.len().min(1)], Path::new(&store.to_string()));
    }
}

struct FilterIvars {
    recommended: Vec<PathBuf>,
    all: Cell<bool>,
    panel: RefCell<Option<Weak<NSOpenPanel>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGChooseAppFilter"]
    #[ivars = FilterIvars]
    struct Filter;

    unsafe impl NSObjectProtocol for Filter {}

    unsafe impl NSOpenSavePanelDelegate for Filter {
        #[unsafe(method(panel:shouldEnableURL:))]
        fn should_enable(&self, _: &AnyObject, url: &NSURL) -> Bool {
            let Some(path) = url.path().map(|path| PathBuf::from(path.to_string())) else {
                return Bool::NO;
            };
            Bool::new(match path.extension().is_some_and(|ext| ext == "app") {
                true => self.ivars().all.get() || self.ivars().recommended.contains(&path),
                false => path.is_dir(),
            })
        }
    }

    impl Filter {
        #[unsafe(method(pickFilter:))]
        fn pick_filter(&self, sender: &NSPopUpButton) {
            self.ivars().all.set(sender.indexOfSelectedItem() == 1);
            if let Some(panel) = self.ivars().panel.borrow().as_ref().and_then(Weak::load) {
                panel.validateVisibleColumns();
            }
        }
    }
);

fn choose_message(files: &[PathBuf]) -> String {
    match files {
        [only] => format!(
            "Choose an application to open the document \u{201c}{}\u{201d}.",
            finder_fs::name::of_path(only)
        ),
        many => format!("Choose an application to open {} items.", many.len()),
    }
}

fn accessory(
    mtm: MainThreadMarker,
    filter: &Filter,
    always: bool,
) -> (Retained<NSView>, Retained<NSButton>) {
    let frame = |x: f64, y: f64, width: f64, height: f64| {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    };
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame(0., 0., 380., 64.));
    let label = NSTextField::labelWithString(&NSString::from_str("Enable:"), mtm);
    label.setFrame(frame(40., 37., 60., 17.));
    let pick = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        frame(100., 32., 240., 25.),
        false,
    );
    for title in ["Recommended Applications", "All Applications"] {
        pick.addItemWithTitle(&NSString::from_str(title));
    }
    let target: &AnyObject = filter;
    unsafe {
        pick.setTarget(Some(target));
        pick.setAction(Some(sel!(pickFilter:)));
    }
    let keep = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str("Always Open With"),
            None,
            None,
            mtm,
        )
    };
    keep.setFrame(frame(100., 8., 240., 18.));
    keep.setState(match always {
        true => NSControlStateValueOn,
        false => NSControlStateValueOff,
    });
    view.addSubview(&label);
    view.addSubview(&pick);
    view.addSubview(&keep);
    (view, keep)
}

pub(crate) fn choose_app(files: Vec<PathBuf>, always: bool, cx: &mut Context<FinderDemo>) {
    if files.is_empty() {
        return;
    }
    // From a task, outside GPUI's borrow of the app, which the modal loop would re-enter.
    cx.spawn(async move |_, _| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let recommended = apps_for_all(&files)
            .into_iter()
            .map(|app| app.path)
            .collect();
        let filter = Filter::alloc(mtm).set_ivars(FilterIvars {
            recommended,
            all: Cell::new(false),
            panel: RefCell::new(None),
        });
        let filter: Retained<Filter> = unsafe { msg_send![super(filter), init] };
        let panel = NSOpenPanel::openPanel(mtm);
        *filter.ivars().panel.borrow_mut() = Some(Weak::new(&panel));
        panel.setTitle(Some(&NSString::from_str("Choose Application")));
        panel.setMessage(Some(&NSString::from_str(&choose_message(&files))));
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
            "/Applications",
        ))));
        unsafe { panel.setDelegate(Some(ProtocolObject::from_ref(&*filter))) };
        let (view, keep) = accessory(mtm, &filter, always);
        panel.setAccessoryView(Some(&view));
        panel.setAccessoryViewDisclosed(true);
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(app) = paths(panel.URLs().to_vec()).into_iter().next() else {
            return;
        };
        if keep.state() == NSControlStateValueOn {
            set_default(&files, &app);
        }
        open_with(&files, &app);
    })
    .detach();
}
