use std::cell::RefCell;

use gpui::{Action, SharedString, actions};
use native_gpui::{MenuCommand, TextCommand};

use super::bar::{
    Chosen, CopyNames, CopyPath, Cut, DeselectAll, FindByName, FindHere, MoveHere, Paste,
    SelectAll, ShowClipboard,
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

#[derive(Default)]
struct History {
    undo: Option<SharedString>,
    redo: Option<SharedString>,
}

thread_local! {
    static HISTORY: RefCell<History> = RefCell::default();
}

pub(super) fn remember(demo: &FinderDemo) {
    HISTORY.with_borrow_mut(|history| {
        let mirror = |kept: &mut Option<SharedString>, label: Option<&str>| {
            if kept.as_deref() != label {
                *kept = label.map(|label| label.to_owned().into());
            }
        };
        mirror(&mut history.undo, demo.journal.undo_label());
        mirror(&mut history.redo, demo.journal.redo_label());
    });
}

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

pub(super) fn enabled(action: &dyn Action, chosen: &Chosen) -> Option<bool> {
    let icons = crate::icon_well::history();
    let on = if action.partial_eq(&UndoOp) {
        icons.map_or_else(
            || HISTORY.with_borrow(|history| history.undo.is_some()),
            |(undo, _)| undo.is_some(),
        )
    } else if action.partial_eq(&RedoOp) {
        icons.map_or_else(
            || HISTORY.with_borrow(|history| history.redo.is_some()),
            |(_, redo)| redo.is_some(),
        )
    } else if action.partial_eq(&Cut) || action.partial_eq(&CopyLink) {
        false
    } else if action.partial_eq(&CopyNames) || action.partial_eq(&DeselectAll) {
        chosen.count > 0
    } else if action.partial_eq(&Paste) || action.partial_eq(&PasteExactly) {
        chosen.on_disk && held().files > 0
    } else if action.partial_eq(&MoveHere) {
        chosen.on_disk && held().movable
    } else if action.partial_eq(&CopyPath) {
        chosen.count > 0 || chosen.on_disk
    } else {
        return None;
    };
    Some(on)
}

pub(super) fn titled(action: &dyn Action, chosen: &Chosen) -> Option<SharedString> {
    let labelled = |verb: &'static str, label: &Option<SharedString>| {
        label
            .as_ref()
            .map_or_else(|| verb.into(), |label| format!("{verb} {label}").into())
    };
    let icons = crate::icon_well::history();
    let title = if action.partial_eq(&UndoOp) {
        match icons {
            Some((undo, _)) => labelled("Undo", &undo.map(SharedString::new_static)),
            None => HISTORY.with_borrow(|history| labelled("Undo", &history.undo)),
        }
    } else if action.partial_eq(&RedoOp) {
        match icons {
            Some((_, redo)) => labelled("Redo", &redo.map(SharedString::new_static)),
            None => HISTORY.with_borrow(|history| labelled("Redo", &history.redo)),
        }
    } else if action.partial_eq(&CopyNames) {
        counted(
            chosen.count,
            "Copy",
            || format!("Copy “{}”", chosen.name),
            |count| format!("Copy {count} Items"),
        )
    } else if action.partial_eq(&CopyPath) {
        let name = match chosen.count {
            0 => &chosen.folder,
            _ => &chosen.name,
        };
        counted(
            chosen.count.max(1),
            "",
            || format!("Copy “{name}” as Pathname"),
            |count| format!("Copy {count} Items as Pathnames"),
        )
    } else if action.partial_eq(&Paste) {
        counted(
            held().files,
            "Paste",
            || "Paste Item".into(),
            |count| format!("Paste {count} Items"),
        )
    } else if action.partial_eq(&PasteExactly) {
        counted(
            held().files,
            "Paste Exactly",
            || "Paste Item Exactly".into(),
            |count| format!("Paste {count} Items Exactly"),
        )
    } else if action.partial_eq(&MoveHere) {
        let held = held();
        match (held.movable, held.files) {
            (false, _) => "Move Items Here".into(),
            (true, 1) => "Move Item Here".into(),
            (true, count) => format!("Move {count} Items Here").into(),
        }
    } else {
        return None;
    };
    Some(title)
}
