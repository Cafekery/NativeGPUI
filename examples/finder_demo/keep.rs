use std::cell::Cell;
use std::path::Path;
use std::ptr::NonNull;

use block2::StackBlock;
use gpui::{AnyWindowHandle, App, Bounds, WindowBounds, WindowHandle, point, px, size};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSApplication, NSMenuDidSendActionNotification, NSMenuItem, NSMenuWillSendActionNotification,
    NSWindow,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSProcessInfo, NSString,
    NSUserDefaults,
};

use crate::app::FinderDemo;
use crate::data::intern;
use crate::frame::ns_window;
use crate::{finder_window, open_finder_tab};

const KEPT: &str = "ng.finder.kept";

thread_local! {
    static QUIT_ALTERNATE: Cell<Option<bool>> = const { Cell::new(None) };
}

struct Kept {
    group: usize,
    selected: bool,
    view: usize,
    bounds: WindowBounds,
    folder: &'static str,
}

/// AppKit's own launch restore would order a reopened tab's host window back in beside its group.
pub(crate) fn skip_appkit_restore() {
    let key = NSString::from_str("ApplePersistenceIgnoreState");
    let yes = NSNumber::new_bool(true);
    let defaults = defaults();
    unsafe {
        defaults.registerDefaults(&NSDictionary::from_slices(&[&*key], &[&*yes as &AnyObject]))
    };
}

/// Quit's ⌥ alternate, added by AppKit, inverts `NSQuitAlwaysKeepsWindows` and sends `terminate:`.
pub(crate) fn follow_quit(cx: &mut App) {
    let center = NSNotificationCenter::defaultCenter();
    let sending = StackBlock::new(|note: NonNull<NSNotification>| {
        let item = unsafe { note.as_ref() }
            .userInfo()
            .and_then(|info| info.objectForKey(&NSString::from_str("MenuItem")))
            .and_then(|item| item.downcast::<NSMenuItem>().ok());
        if let Some(item) = item.filter(|item| item.action() == Some(sel!(terminate:))) {
            QUIT_ALTERNATE.set(Some(item.isAlternate()));
            // Otherwise `terminate:` may kill the app without ever running `on_app_quit`.
            NSProcessInfo::processInfo().disableSuddenTermination();
        }
    })
    .copy();
    let sent = StackBlock::new(|_: NonNull<NSNotification>| {
        if QUIT_ALTERNATE.take().is_some() {
            NSProcessInfo::processInfo().enableSuddenTermination();
        }
    })
    .copy();
    for (name, block) in [
        (unsafe { NSMenuWillSendActionNotification }, &sending),
        (unsafe { NSMenuDidSendActionNotification }, &sent),
    ] {
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, block)
        };
        std::mem::forget(observer);
    }
    cx.on_app_quit(|cx| {
        let always = defaults().boolForKey(&NSString::from_str("NSQuitAlwaysKeepsWindows"));
        match always != QUIT_ALTERNATE.get().unwrap_or(false) {
            true => save(cx),
            false => clear(),
        }
        async {}
    })
    .detach();
}

pub(crate) fn reopen(cx: &mut App) -> bool {
    let kept: Vec<Kept> = defaults()
        .stringArrayForKey(&NSString::from_str(KEPT))
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| parse(&line.to_string()))
                .collect()
        })
        .unwrap_or_default();
    clear();
    let mut groups: Vec<Vec<Kept>> = Vec::new();
    for kept in kept
        .into_iter()
        .filter(|kept| !kept.folder.starts_with('/') || Path::new(kept.folder).exists())
    {
        match groups.last_mut() {
            Some(group) if group[0].group == kept.group => group.push(kept),
            _ => groups.push(vec![kept]),
        }
    }
    if groups.is_empty() {
        return false;
    }
    let mut fronts: Vec<AnyWindowHandle> = Vec::new();
    for group in groups.iter().rev() {
        let bounds = group[0].bounds;
        let mut front = None;
        let mut host: Option<AnyWindowHandle> = None;
        for kept in group {
            let handle = match host {
                None => finder_window(kept.folder, bounds, false, true, cx),
                Some(_) => open_finder_tab(kept.folder, bounds, cx),
            };
            join(handle, host, kept.view, cx);
            host = Some(handle.into());
            if kept.selected {
                front = host;
            }
        }
        fronts.extend(front.or(host));
    }
    cx.spawn(async move |cx| {
        for front in fronts {
            front
                .update(cx, |_, window, _| {
                    if let Some(window) = ns_window(window) {
                        window.makeKeyAndOrderFront(None);
                    }
                })
                .ok();
        }
    })
    .detach();
    true
}

fn join(tab: WindowHandle<FinderDemo>, host: Option<AnyWindowHandle>, view: usize, cx: &mut App) {
    tab.update(cx, |demo, window, cx| {
        if demo.view != view {
            demo.browse_as(view, window, cx);
        }
        if let Some(host) = host {
            native_gpui::add_tab(host, window, cx);
        }
    })
    .ok();
}

fn save(cx: &mut App) {
    let mut open: Vec<(Retained<NSWindow>, String)> = Vec::new();
    for handle in cx.windows() {
        let Some(finder) = handle.downcast::<FinderDemo>() else {
            continue;
        };
        let read = finder.update(cx, |demo, window, _| {
            let ns = ns_window(window)?;
            let (state, bounds) = match window.window_bounds() {
                WindowBounds::Windowed(bounds) => ("w", bounds),
                WindowBounds::Maximized(bounds) => ("m", bounds),
                WindowBounds::Fullscreen(bounds) => ("f", bounds),
            };
            let line = format!(
                "{}\t{state}\t{}\t{}\t{}\t{}\t{}",
                demo.view,
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                demo.here(),
            );
            Some((ns, line))
        });
        open.extend(read.ok().flatten());
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let mut fronts: Vec<Retained<NSWindow>> = NSApplication::sharedApplication(mtm)
        .orderedWindows()
        .iter()
        .collect();
    fronts.extend(open.iter().map(|(window, _)| window.clone()));
    let mut lines = Vec::new();
    let mut group = 0;
    for front in fronts {
        let tabs = front
            .tabbedWindows()
            .map(|tabs| tabs.iter().collect())
            .unwrap_or_else(|| vec![front.clone()]);
        let mut any = false;
        for tab in tabs {
            let Some(at) = open.iter().position(|(window, _)| *window == tab) else {
                continue;
            };
            let (_, line) = open.swap_remove(at);
            lines.push(format!("{group}\t{}\t{line}", u8::from(tab == front)));
            any = true;
        }
        group += usize::from(any);
    }
    let lines: Vec<_> = lines.iter().map(|line| NSString::from_str(line)).collect();
    let lines = NSArray::from_retained_slice(&lines);
    let defaults = defaults();
    unsafe { defaults.setObject_forKey(Some(&lines), &NSString::from_str(KEPT)) };
    defaults.synchronize();
}

fn parse(line: &str) -> Option<Kept> {
    let mut fields = line.splitn(9, '\t');
    let mut next = || fields.next();
    let group = next()?.parse().ok()?;
    let selected = next()? == "1";
    let view = next()?.parse().ok()?;
    let state = next()?;
    let mut number = || next()?.parse::<f32>().ok();
    let bounds = Bounds {
        origin: point(px(number()?), px(number()?)),
        size: size(px(number()?), px(number()?)),
    };
    let bounds = match state {
        "m" => WindowBounds::Maximized(bounds),
        "f" => WindowBounds::Fullscreen(bounds),
        _ => WindowBounds::Windowed(bounds),
    };
    Some(Kept {
        group,
        selected,
        view,
        bounds,
        folder: intern(next()?),
    })
}

fn clear() {
    let defaults = defaults();
    defaults.removeObjectForKey(&NSString::from_str(KEPT));
    defaults.synchronize();
}

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}
