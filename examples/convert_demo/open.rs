use std::cell::RefCell;
use std::path::{Path, PathBuf};

use block2::RcBlock;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSWorkspace};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};

// begin, not runModal: a nested run loop re-enters GPUI while App is still borrowed.
pub fn pick_files(done: impl FnOnce(Vec<PathBuf>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(Vec::new());
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(true);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    let retained = panel.clone();
    let done = RefCell::new(Some(done));
    let block = RcBlock::new(move |response| {
        let Some(done) = done.borrow_mut().take() else {
            return;
        };
        let paths = if response == NSModalResponseOK {
            retained
                .URLs()
                .iter()
                .filter_map(|url| url.path().map(|p| PathBuf::from(p.to_string())))
                .collect()
        } else {
            Vec::new()
        };
        done(paths);
    });
    panel.beginWithCompletionHandler(&block);
}

pub fn pick_folder(done: impl FnOnce(Option<PathBuf>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(None);
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setCanCreateDirectories(true);
    panel.setPrompt(Some(&NSString::from_str("Choose")));
    let retained = panel.clone();
    let done = RefCell::new(Some(done));
    let block = RcBlock::new(move |response| {
        let Some(done) = done.borrow_mut().take() else {
            return;
        };
        let path = (response == NSModalResponseOK)
            .then(|| {
                retained
                    .URL()
                    .and_then(|url| url.path())
                    .map(|path| PathBuf::from(path.to_string()))
            })
            .flatten();
        done(path);
    });
    panel.beginWithCompletionHandler(&block);
}

pub fn reveal(path: &Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}
