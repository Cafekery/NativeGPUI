use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, thread};

use block2::DynBlock;
use finder_fs::ops::Op;
use finder_sftp::Error;
use gpui::{AsyncApp, Context, SharedString, WeakEntity, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread as _, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSImage, NSPasteboard,
    NSPasteboardItem, NSPasteboardItemDataProvider, NSPasteboardType, NSPasteboardTypeFileURL,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL};
use parking_lot::{Condvar, Mutex};

use super::preview::fresh;
use super::{Then, connected, jobs::cached, split};
use crate::app::FinderDemo;
use crate::data::{Row, split_key};
use crate::ops::After;

const DRAG_ICON: f64 = 32.;

struct Promised {
    id: PathBuf,
    name: String,
    local: PathBuf,
    fetched: Arc<Fetch>,
    demo: WeakEntity<FinderDemo>,
    app: AsyncApp,
}

#[derive(Default)]
struct Fetch {
    done: Mutex<Option<bool>>,
    ready: Condvar,
}

impl Fetch {
    fn finish(&self, ok: bool) {
        *self.done.lock() = Some(ok);
        self.ready.notify_all();
    }

    fn wait(&self) -> bool {
        let mut done = self.done.lock();
        while done.is_none() {
            self.ready.wait(&mut done);
        }
        done.unwrap_or(false)
    }
}

thread_local! {
    static PROMISES: RefCell<Vec<Retained<Promise>>> = const { RefCell::new(Vec::new()) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGRemotePromise"]
    #[ivars = Promised]
    struct Promise;

    unsafe impl NSObjectProtocol for Promise {}

    unsafe impl NSFilePromiseProviderDelegate for Promise {
        #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
        fn file_name(&self, _: &NSFilePromiseProvider, _: &NSString) -> Retained<NSString> {
            NSString::from_str(&self.ivars().name)
        }

        #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
        fn write(
            &self,
            _: &NSFilePromiseProvider,
            url: &NSURL,
            done: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let done = done.copy();
            let Some(to) = url.to_file_path() else {
                return promised(&done, false);
            };
            let Promised { id, demo, app, .. } = self.ivars();
            let (id, demo) = (id.clone(), demo.clone());
            app.spawn(async move |cx| {
                let into = to.parent().unwrap_or(&to).to_path_buf();
                let op = Op::Copy {
                    items: vec![id.clone()],
                    into,
                };
                let then = Some(Then::Promise(done.clone()));
                let started = demo.update(cx, |demo, cx| {
                    demo.submit_remote(op, vec![(id, to)], then, After::Nothing, cx)
                });
                if started.is_err() {
                    promised(&done, false);
                }
            })
            .detach();
        }
    }

    /// A plain file URL to the cached copy, for receivers that know no promises.
    unsafe impl NSPasteboardItemDataProvider for Promise {
        #[unsafe(method(pasteboard:item:provideDataForType:))]
        fn provide(
            &self,
            _: Option<&NSPasteboard>,
            item: &NSPasteboardItem,
            kind: &NSPasteboardType,
        ) {
            let Promised { local, fetched, .. } = self.ivars();
            let Some(path) = local.to_str().filter(|_| fetched.wait()) else {
                return;
            };
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            if let Some(url) = url.absoluteString() {
                item.setString_forType(&url, kind);
            }
        }
    }

    unsafe impl NSDraggingSource for Promise {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(&self, _: &NSDraggingSession, _: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }
);

impl FinderDemo {
    pub(crate) fn drag_out(
        &self,
        keys: &[SharedString],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some(view) = crate::frame::ns_view(window) else {
            return false;
        };
        let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
            return false;
        };
        let at = event.locationInWindow();
        let frame = NSRect::new(
            NSPoint::new(at.x - DRAG_ICON / 2., at.y - DRAG_ICON / 2.),
            NSSize::new(DRAG_ICON, DRAG_ICON),
        );
        let mut promises = Vec::new();
        let mut items = Vec::new();
        let mut copies = Vec::new();
        for key in keys {
            let Some(row) = split_key(key).and_then(|(dir, _)| self.remote_row(dir, key)) else {
                continue;
            };
            let Some(local) = cached(key) else {
                continue;
            };
            let fetched = Arc::new(Fetch::default());
            copies.push((key.clone(), row.clone(), local.clone(), fetched.clone()));
            let ivars = Promised {
                id: PathBuf::from(key.as_ref()),
                name: row.name.to_string(),
                local,
                fetched,
                demo: cx.entity().downgrade(),
                app: cx.to_async(),
            };
            let promise: Retained<Promise> =
                unsafe { msg_send![super(Promise::alloc(mtm).set_ivars(ivars)), init] };
            let kind = NSString::from_str(&row.uti);
            let provider = NSFilePromiseProvider::initWithFileType_delegate(
                NSFilePromiseProvider::alloc(),
                &kind,
                ProtocolObject::from_ref(&*promise),
            );
            // Undocumented, so a provider that lost it still drags as a promise alone.
            if provider.respondsToSelector(sel!(setDataProvider:forTypes:)) {
                let source =
                    ProtocolObject::<dyn NSPasteboardItemDataProvider>::from_ref(&*promise);
                let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
                let _: () =
                    unsafe { msg_send![&*provider, setDataProvider: source, forTypes: &*types] };
            }
            let item = NSDraggingItem::initWithPasteboardWriter(
                NSDraggingItem::alloc(),
                ProtocolObject::from_ref(&*provider),
            );
            let icon = icon(&kind);
            let icon: Option<&AnyObject> = icon.as_deref().map(AsRef::as_ref);
            unsafe { item.setDraggingFrame_contents(frame, icon) };
            items.push(item);
            promises.push(promise);
        }
        let Some(source) = promises.first().cloned() else {
            return false;
        };
        thread::spawn(move || {
            for (id, row, local, fetched) in copies {
                fetched.finish(whole(&id, &row, &local));
            }
        });
        view.beginDraggingSessionWithItems_event_source(
            &NSArray::from_retained_slice(&items),
            &event,
            ProtocolObject::from_ref(&*source),
        );
        // A provider holds its delegate weakly, and AppKit does not retain the dragging source.
        PROMISES.set(promises);
        true
    }
}

/// The whole file in the cache, downloaded now if the copy there is stale.
pub(crate) fn whole(id: &str, row: &Row, local: &Path) -> bool {
    fresh(local, row, true) || download(id, local).is_ok()
}

fn download(id: &str, local: &Path) -> Result<(), Error> {
    let server = connected(id).ok_or(Error::Disconnected)?;
    if let Some(parent) = local.parent() {
        fs::create_dir_all(parent)?;
    }
    server.server.download(split(id).1, local, &mut |_, _| true)
}

fn icon(uti: &NSString) -> Option<Retained<NSImage>> {
    let class = AnyClass::get(c"UTType")?;
    let kind: Option<Retained<AnyObject>> = unsafe { msg_send![class, typeWithIdentifier: uti] };
    let workspace = NSWorkspace::sharedWorkspace();
    Some(unsafe { msg_send![&*workspace, iconForContentType: &*kind?] })
}

pub(super) fn promised(done: &DynBlock<dyn Fn(*mut NSError)>, written: bool) {
    let error = (!written).then(|| {
        let domain = unsafe { objc2_foundation::NSCocoaErrorDomain };
        NSError::new(objc2_foundation::NSFileWriteUnknownError, domain)
    });
    let error = error.as_deref().map_or(std::ptr::null_mut(), |error| {
        std::ptr::from_ref(error).cast_mut()
    });
    done.call((error,));
}
