use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingInfo, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSOutlineView, NSOutlineViewDropOnItemIndex,
    NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSInteger, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL,
};

use super::{Shared, SidebarDrop, SidebarMove};
use crate::bridge::appkit::gpui_view;

pub(super) const ROW_PASTEBOARD_TYPE: &str = "com.native-gpui.sidebar-row";

pub(super) fn dragged_key(info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<u64> {
    let text = info
        .draggingPasteboard()
        .stringForType(&NSString::from_str(ROW_PASTEBOARD_TYPE))?;
    text.to_string().parse().ok()
}

pub(super) fn dragged_paths(info: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let pasteboard = info.draggingPasteboard();
    let Some(items) = pasteboard.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            let url = NSURL::URLWithString(&text)?;
            Some(PathBuf::from(url.path()?.to_string()))
        })
        .collect()
}

pub(super) fn file_drop_target(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> Option<(u64, usize)> {
    let (parent, at) = drop_target(shared, outline, info, item, index)?;
    let parent = parent?;
    shared
        .model
        .borrow()
        .node(parent)
        .filter(|node| node.group)
        .map(|_| (parent, at))
}

/// Finder makes a sidebar row of a folder and of nothing else: a file dropped between rows shows
/// no insertion line.
pub(super) fn insertable(paths: &[PathBuf]) -> bool {
    paths.iter().all(|path| path.is_dir())
}

/// A row that takes files itself, as Finder's tags do: the drop lands on it, not between rows.
pub(super) fn file_drop_row(shared: &Rc<Shared>, item: Option<&AnyObject>) -> Option<u64> {
    let key = Shared::key_of_item(item?);
    shared
        .model
        .borrow()
        .node(key)
        .filter(|node| node.drops)
        .map(|_| key)
}

pub(super) fn validate_file_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> NSDragOperation {
    let paths = dragged_paths(info);
    if shared.on_drop_paths.borrow().is_none() || paths.is_empty() {
        return NSDragOperation::empty();
    }
    if let Some(key) = file_drop_row(shared, item) {
        unsafe {
            outline.setDropItem_dropChildIndex(
                Some(&shared.item(key)),
                NSOutlineViewDropOnItemIndex as NSInteger,
            )
        };
        return NSDragOperation::Copy;
    }
    if !insertable(&paths) {
        return NSDragOperation::empty();
    }
    let Some((group, at)) = file_drop_target(shared, outline, info, item, index) else {
        return NSDragOperation::empty();
    };
    unsafe { outline.setDropItem_dropChildIndex(Some(&shared.item(group)), at as NSInteger) };
    NSDragOperation::Generic
}

pub(super) fn accept_file_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> bool {
    let paths = dragged_paths(info);
    if paths.is_empty() {
        return false;
    }
    let onto = file_drop_row(shared, item);
    if onto.is_none() && !insertable(&paths) {
        return false;
    }
    let Some((group, at)) = onto
        .and_then(|key| {
            let model = shared.model.borrow();
            let parent = model.parent_key(key)?;
            let at = model
                .children_keys(Some(parent))
                .iter()
                .position(|&k| k == key)?;
            Some((parent, at))
        })
        .or_else(|| file_drop_target(shared, outline, info, item, index))
    else {
        return false;
    };
    let Some(handler) = shared.on_drop_paths.borrow().clone() else {
        return false;
    };
    let model_ids = {
        let model = shared.model.borrow();
        (model.id_for(group), onto.and_then(|key| model.id_for(key)))
    };
    let (Some(group), onto) = model_ids else {
        return false;
    };
    let dropped = SidebarDrop {
        paths,
        group,
        index: at,
        onto,
    };
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| handler(&dropped, window, app));
        })
        .detach();
    true
}

pub(super) fn drop_target(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> Option<(Option<u64>, usize)> {
    if index != NSOutlineViewDropOnItemIndex as NSInteger {
        return Some((item.map(Shared::key_of_item), index.max(0) as usize));
    }
    let key = Shared::key_of_item(item?);
    let model = shared.model.borrow();
    if model.node(key)?.group {
        return Some((Some(key), 0));
    }
    let parent = model.parent_key(key);
    let at = model.children_keys(parent).iter().position(|&k| k == key)?;
    let row = unsafe { outline.rowForItem(item) };
    let rect = outline.rectOfRow(row);
    let point = outline.convertPoint_fromView(info.draggingLocation(), None);
    // An outline view is flipped, so a larger y is further down the list.
    let below = point.y > rect.origin.y + rect.size.height / 2.;
    Some((parent, at + usize::from(below)))
}

pub(super) fn row_pasteboard_item(
    shared: &Rc<Shared>,
    item: &AnyObject,
) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
    let key = Shared::key_of_item(item);
    if !shared
        .model
        .borrow()
        .node(key)
        .is_some_and(|node| !node.group)
    {
        return None;
    }
    let entry = NSPasteboardItem::new();
    entry.setString_forType(
        &NSString::from_str(&key.to_string()),
        &NSString::from_str(ROW_PASTEBOARD_TYPE),
    );
    Some(ProtocolObject::from_retained(entry))
}

pub(super) fn validate_row_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> NSDragOperation {
    let Some(source) = dragged_key(info) else {
        return validate_file_drop(shared, outline, info, item, index);
    };
    let Some((parent, at)) = drop_target(shared, outline, info, item, index) else {
        return NSDragOperation::empty();
    };
    if shared.model.borrow().parent_key(source) != parent {
        return NSDragOperation::empty();
    }
    let parent_item = parent.map(|key| shared.item(key));
    unsafe { outline.setDropItem_dropChildIndex(parent_item.as_deref(), at as NSInteger) };
    NSDragOperation::Move
}

pub(super) fn accept_row_drop(
    shared: &Rc<Shared>,
    outline: &NSOutlineView,
    info: &ProtocolObject<dyn NSDraggingInfo>,
    item: Option<&AnyObject>,
    index: NSInteger,
) -> bool {
    let Some(source) = dragged_key(info) else {
        return accept_file_drop(shared, outline, info, item, index);
    };
    if index < 0 {
        return false;
    }
    let parent = item.map(Shared::key_of_item);
    let from = {
        let model = shared.model.borrow();
        if model.parent_key(source) != parent {
            return false;
        }
        match model
            .children_keys(parent)
            .iter()
            .position(|&k| k == source)
        {
            Some(at) => at,
            None => return false,
        }
    };
    // The drop index counts the row itself; both moves count the list with the row lifted out.
    let to = (index as usize).saturating_sub(usize::from(index as usize > from));
    if to == from {
        return true;
    }
    if !shared.model.borrow_mut().move_child(parent, from, to) {
        return false;
    }
    shared.writes.set(shared.writes.get() + 1);
    let parent_item = parent.map(|key| shared.item(key));
    unsafe {
        outline.moveItemAtIndex_inParent_toIndex_inParent(
            from as NSInteger,
            parent_item.as_deref(),
            to as NSInteger,
            parent_item.as_deref(),
        );
    }
    let Some(handler) = shared.on_reorder.borrow().clone() else {
        return true;
    };
    let moved = {
        let model = shared.model.borrow();
        SidebarMove {
            id: model.id_for(source).unwrap_or_default(),
            parent: parent.and_then(|key| model.id_for(key)),
            index: to,
        }
    };
    shared
        .cx
        .borrow()
        .spawn(async move |cx| {
            let _ = cx.update(|window, app| handler(&moved, window, app));
        })
        .detach();
    true
}

/// Hands the drag GPUI is running to AppKit as file URLs: a hosted `NSOutlineView` is a dragging
/// destination and sees nothing of GPUI's own drags.
pub fn drag_files(window: &mut Window, paths: &[PathBuf]) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let view = gpui_view(window);
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return false;
    };
    let at = event.locationInWindow();
    let workspace = NSWorkspace::sharedWorkspace();
    let items: Vec<Retained<NSDraggingItem>> = paths
        .iter()
        .filter_map(|path| {
            let text = NSString::from_str(path.to_str()?);
            let url = NSURL::fileURLWithPath(&text);
            let item = NSDraggingItem::initWithPasteboardWriter(
                NSDraggingItem::alloc(),
                ProtocolObject::from_ref(&*url),
            );
            let frame = NSRect::new(
                NSPoint::new(at.x - DRAG_ICON / 2., at.y - DRAG_ICON / 2.),
                NSSize::new(DRAG_ICON, DRAG_ICON),
            );
            let icon = workspace.iconForFile(&text);
            unsafe { item.setDraggingFrame_contents(frame, Some(&icon)) };
            Some(item)
        })
        .collect();
    if items.is_empty() {
        return false;
    }
    let source = FileDragSource::new(mtm);
    view.beginDraggingSessionWithItems_event_source(
        &NSArray::from_retained_slice(&items),
        &event,
        ProtocolObject::from_ref(&*source),
    );
    // AppKit does not keep the source alive; the session ends when the next one starts.
    DRAG_SOURCE.with(|held| *held.borrow_mut() = Some(source));
    true
}

const DRAG_ICON: f64 = 32.;

thread_local! {
    static DRAG_SOURCE: RefCell<Option<Retained<FileDragSource>>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGSidebarFileDragSource"]
    struct FileDragSource;

    unsafe impl NSObjectProtocol for FileDragSource {}

    unsafe impl NSDraggingSource for FileDragSource {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _: &NSDraggingSession,
            context: NSDraggingContext,
        ) -> NSDragOperation {
            match context {
                NSDraggingContext::WithinApplication => {
                    NSDragOperation::Generic | NSDragOperation::Move | NSDragOperation::Copy
                }
                _ => NSDragOperation::Copy,
            }
        }
    }
);

impl FileDragSource {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}
