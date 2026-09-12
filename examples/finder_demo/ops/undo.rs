use std::cell::{Cell, RefCell};

use finder_fs::ops::Conflict;
use finder_fs::queue::Journal;
use gpui::{Action, App, Context, Div, InteractiveElement, actions};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};

use super::After;
use super::jobs::Record;
use crate::app::FinderDemo;

actions!(finder_demo, [UndoOp, RedoOp]);

fn find_submenu(bar: &NSMenu, title: &str) -> Option<Retained<NSMenu>> {
    bar.itemArray()
        .iter()
        .find(|item| item.title().to_string() == title)
        .and_then(|item| item.submenu())
}

fn find_item(menu: &NSMenu, title: &str) -> Option<Retained<NSMenuItem>> {
    menu.itemArray()
        .iter()
        .find(|item| item.title().to_string() == title)
}

struct SinkIvars {
    cx: gpui::AsyncApp,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NGUndoRedoSink"]
    #[ivars = SinkIvars]
    struct UndoRedoSink;

    impl UndoRedoSink {
        #[unsafe(method(ngUndo:))]
        fn ng_undo(&self, _sender: *mut AnyObject) {
            self.dispatch(UndoOp);
        }

        #[unsafe(method(ngRedo:))]
        fn ng_redo(&self, _sender: *mut AnyObject) {
            self.dispatch(RedoOp);
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: *mut NSMenuItem) -> bool {
            let action = unsafe { item.as_ref() }.and_then(|item| item.action());
            STATE.with(|state| {
                let (can_undo, can_redo) = state.get();
                if action == Some(sel!(ngUndo:)) {
                    can_undo
                } else if action == Some(sel!(ngRedo:)) {
                    can_redo
                } else {
                    true
                }
            })
        }
    }
);

impl UndoRedoSink {
    fn dispatch(&self, action: impl Action) {
        let boxed: Box<dyn Action> = Box::new(action);
        self.ivars()
            .cx
            .clone()
            .spawn(async move |cx| {
                cx.update(|cx| cx.dispatch_action(&*boxed));
            })
            .detach();
    }
}

thread_local! {
    static STATE: Cell<(bool, bool)> = const { Cell::new((false, false)) };
    static ITEMS: RefCell<Option<(Retained<NSMenuItem>, Retained<NSMenuItem>)>> =
        const { RefCell::new(None) };
    static SINK: RefCell<Option<Retained<UndoRedoSink>>> = const { RefCell::new(None) };
}

/// Call right after `install_menus`, while the Edit menu still has its stock Undo and Redo rows.
pub(crate) fn install_undo_redo(cx: &mut App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(bar) = NSApplication::sharedApplication(mtm).mainMenu() else {
        return;
    };
    let Some(edit) = find_submenu(&bar, "Edit") else {
        return;
    };
    let (Some(undo_item), Some(redo_item)) = (find_item(&edit, "Undo"), find_item(&edit, "Redo"))
    else {
        return;
    };
    let sink = UndoRedoSink::alloc(mtm).set_ivars(SinkIvars { cx: cx.to_async() });
    let sink: Retained<UndoRedoSink> = unsafe { msg_send![super(sink), init] };
    unsafe {
        undo_item.setTarget(Some(&sink));
        undo_item.setAction(Some(sel!(ngUndo:)));
        redo_item.setTarget(Some(&sink));
        redo_item.setAction(Some(sel!(ngRedo:)));
    }
    SINK.with(|shared| *shared.borrow_mut() = Some(sink));
    ITEMS.with(|shared| *shared.borrow_mut() = Some((undo_item, redo_item)));
}

pub(crate) fn refresh_edit_menu(journal: &Journal) {
    ITEMS.with(|shared| {
        let items = shared.borrow();
        let Some((undo_item, redo_item)) = items.as_ref() else {
            return;
        };
        let (undo_title, can_undo) = match journal.undo_label() {
            Some(label) => (format!("Undo {label}"), true),
            None => ("Undo".to_string(), false),
        };
        let (redo_title, can_redo) = match journal.redo_label() {
            Some(label) => (format!("Redo {label}"), true),
            None => ("Redo".to_string(), false),
        };
        undo_item.setTitle(&NSString::from_str(&undo_title));
        redo_item.setTitle(&NSString::from_str(&redo_title));
        STATE.with(|state| state.set((can_undo, can_redo)));
    });
}

impl FinderDemo {
    pub(crate) fn perform_undo(&mut self, cx: &mut Context<Self>) {
        let Some(undoable) = self.journal.pop_undo() else {
            return;
        };
        refresh_edit_menu(&self.journal);
        // Whatever took an undone item's place since is kept, never replaced.
        self.submit(
            undoable.undo,
            Conflict::KeepBoth,
            After::Select,
            Record::Undo,
            cx,
        );
    }

    pub(crate) fn perform_redo(&mut self, cx: &mut Context<Self>) {
        let Some(undoable) = self.journal.pop_redo() else {
            return;
        };
        refresh_edit_menu(&self.journal);
        let at = self.journal.undo_len() - 1;
        self.submit(
            undoable.redo,
            Conflict::KeepBoth,
            After::Select,
            Record::Redo(at),
            cx,
        );
    }

    pub(crate) fn wire_undo_redo(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &UndoOp, _, cx| this.perform_undo(cx)))
            .on_action(cx.listener(|this, _: &RedoOp, _, cx| this.perform_redo(cx)))
    }
}
