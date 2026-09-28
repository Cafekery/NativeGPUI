use std::cell::RefCell;

use gpui::{Action, SharedString, actions};
use native_gpui::{MenuCommand, TextCommand};

use super::bar::{
    CopyNames, CopyPath, Cut, DeselectAll, FindByName, FindHere, MoveHere, Paste, SelectAll,
    ShowClipboard,
};
use crate::app::FinderDemo;
use crate::ops::clipboard::held;
use crate::ops::undo::{RedoOp, UndoOp};

actions!(finder_demo, [CopyLink, PasteExactly]);

pub(crate) fn rows() -> Vec<MenuCommand> {
    vec![
        MenuCommand::new("Undo", UndoOp)
            .key("cmd-z")
            .edits_text(TextCommand::Undo),
        MenuCommand::new("Redo", RedoOp)
            .key("cmd-shift-z")
            .edits_text(TextCommand::Redo),
        MenuCommand::separator(),
        MenuCommand::new("Cut", Cut)
            .key("cmd-x")
            .edits_text(TextCommand::Cut),
        MenuCommand::new("Copy", CopyNames)
            .key("cmd-c")
            .edits_text(TextCommand::Copy),
        MenuCommand::new("Copy as Pathname", CopyPath)
            .key("cmd-alt-c")
            .option_alternate(),
        MenuCommand::new("Copy as Link", CopyLink)
            .key("cmd-ctrl-alt-c")
            .option_alternate(),
        MenuCommand::new("Paste", Paste)
            .key("cmd-v")
            .edits_text(TextCommand::Paste),
        MenuCommand::new("Move Items Here", MoveHere)
            .key("cmd-alt-v")
            .option_alternate(),
        MenuCommand::new("Paste Exactly", PasteExactly)
            .key("cmd-alt-shift-v")
            .option_alternate(),
        MenuCommand::new("Select All", SelectAll)
            .key("cmd-a")
            .edits_text(TextCommand::SelectAll),
        MenuCommand::new("Deselect All", DeselectAll)
            .key("cmd-alt-a")
            .option_alternate(),
        MenuCommand::separator(),
        MenuCommand::new("Show Clipboard", ShowClipboard),
        MenuCommand::separator(),
        MenuCommand::new("Search", FindHere).key("cmd-f"),
        MenuCommand::new("Search by Name…", FindByName)
            .key("cmd-ctrl-shift-f")
            .option_alternate(),
    ]
}

/// Menu validation cannot read the window, so the Edit rows follow this mirror of the main one.
#[derive(Default)]
struct Front {
    seen: (u64, u64, usize),
    chosen: usize,
    first: SharedString,
    folder: SharedString,
    /// The window shows a folder on disk, which a paste can land in.
    on_disk: bool,
    undo: Option<SharedString>,
    redo: Option<SharedString>,
}

thread_local! {
    static FRONT: RefCell<Front> = RefCell::default();
}

pub(crate) fn remember(demo: &FinderDemo) {
    FRONT.with_borrow_mut(|front| {
        let seen = (demo.selected.rev(), demo.folders.changed(), demo.view);
        if front.seen != seen {
            let chosen = demo.chosen_lines();
            front.seen = seen;
            front.chosen = chosen.len();
            front.first = chosen
                .first()
                .map(|line| line.row.display.clone())
                .unwrap_or_default();
            front.folder = demo
                .folder()
                .map(|folder| folder.title)
                .unwrap_or_default()
                .into();
            front.on_disk = std::path::Path::new(demo.target()).is_absolute();
        }
        if front.undo.as_deref() != demo.journal.undo_label() {
            front.undo = demo
                .journal
                .undo_label()
                .map(|label| label.to_owned().into());
        }
        if front.redo.as_deref() != demo.journal.redo_label() {
            front.redo = demo
                .journal
                .redo_label()
                .map(|label| label.to_owned().into());
        }
    });
}

/// Finder's plural rule for these rows: nothing, one item by name, or a count.
fn counted(
    count: usize,
    none: &str,
    one: impl FnOnce() -> String,
    many: impl FnOnce(usize) -> String,
) -> SharedString {
    match count {
        0 => none.to_owned(),
        1 => one(),
        _ => many(count),
    }
    .into()
}

pub(crate) fn enabled(action: &dyn Action) -> Option<bool> {
    FRONT.with_borrow(|front| {
        let on = if action.partial_eq(&UndoOp) {
            front.undo.is_some()
        } else if action.partial_eq(&RedoOp) {
            front.redo.is_some()
        } else if action.partial_eq(&Cut) || action.partial_eq(&CopyLink) {
            false
        } else if action.partial_eq(&CopyNames) || action.partial_eq(&DeselectAll) {
            front.chosen > 0
        } else if action.partial_eq(&Paste) || action.partial_eq(&PasteExactly) {
            front.on_disk && held().files > 0
        } else if action.partial_eq(&MoveHere) {
            front.on_disk && held().movable
        } else if action.partial_eq(&CopyPath) {
            front.chosen > 0 || front.on_disk
        } else {
            return None;
        };
        Some(on)
    })
}

pub(crate) fn titled(action: &dyn Action) -> Option<SharedString> {
    FRONT.with_borrow(|front| {
        let files = held().files;
        let title = if action.partial_eq(&UndoOp) {
            front
                .undo
                .as_ref()
                .map_or_else(|| "Undo".into(), |label| format!("Undo {label}").into())
        } else if action.partial_eq(&RedoOp) {
            front
                .redo
                .as_ref()
                .map_or_else(|| "Redo".into(), |label| format!("Redo {label}").into())
        } else if action.partial_eq(&CopyNames) {
            counted(
                front.chosen,
                "Copy",
                || format!("Copy “{}”", front.first),
                |count| format!("Copy {count} Items"),
            )
        } else if action.partial_eq(&CopyPath) {
            let name = match front.chosen {
                0 => &front.folder,
                _ => &front.first,
            };
            counted(
                front.chosen.max(1),
                "",
                || format!("Copy “{name}” as Pathname"),
                |count| format!("Copy {count} Items as Pathnames"),
            )
        } else if action.partial_eq(&Paste) {
            counted(
                files,
                "Paste",
                || "Paste Item".into(),
                |count| format!("Paste {count} Items"),
            )
        } else if action.partial_eq(&PasteExactly) {
            counted(
                files,
                "Paste Exactly",
                || "Paste Item Exactly".into(),
                |count| format!("Paste {count} Items Exactly"),
            )
        } else if action.partial_eq(&MoveHere) {
            match (held().movable, files) {
                (false, _) => "Move Items Here".into(),
                (true, 1) => "Move Item Here".into(),
                (true, count) => format!("Move {count} Items Here").into(),
            }
        } else {
            return None;
        };
        Some(title)
    })
}
