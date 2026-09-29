use std::cell::Cell;
use std::path::{Path, PathBuf};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSModalResponseOK, NSOpenPanel, NSWorkspace};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};

pub fn pick_files(done: impl FnOnce(Vec<PathBuf>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return done(Vec::new());
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(true);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    begin(panel, move |panel, ok| {
        done(if ok {
            panel
                .URLs()
                .iter()
                .filter_map(|url| url_path(&url))
                .collect()
        } else {
            Vec::new()
        })
    });
}

pub fn pick_folder(done: impl FnOnce(Option<PathBuf>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return done(None);
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setAllowsMultipleSelection(false);
    panel.setCanChooseFiles(false);
    panel.setCanChooseDirectories(true);
    panel.setCanCreateDirectories(true);
    panel.setPrompt(Some(&NSString::from_str("Choose")));
    begin(panel, move |panel, ok| {
        done(if ok {
            panel.URL().and_then(|url| url_path(&url))
        } else {
            None
        })
    });
}

// begin, not runModal: a nested run loop re-enters GPUI while App is still borrowed.
fn begin(panel: Retained<NSOpenPanel>, done: impl FnOnce(&NSOpenPanel, bool) + 'static) {
    let retained = panel.clone();
    let done = Cell::new(Some(done));
    let block = RcBlock::new(move |response| {
        if let Some(done) = done.take() {
            done(&retained, response == NSModalResponseOK);
        }
    });
    panel.beginWithCompletionHandler(&block);
}

pub fn url_path(url: &NSURL) -> Option<PathBuf> {
    Some(PathBuf::from(url.path()?.to_string()))
}

pub fn reveal(path: &Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}
