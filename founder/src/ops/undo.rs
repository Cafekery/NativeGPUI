use finder_fs::ops::Conflict;
use gpui::{Context, Div, InteractiveElement, actions};

use super::After;
use super::jobs::Record;
use crate::app::FinderDemo;
use crate::sound::Sound;

actions!(finder_demo, [UndoOp, RedoOp]);

impl FinderDemo {
    fn perform_undo(&mut self, cx: &mut Context<Self>) {
        let Some(undoable) = self.journal.pop_undo() else {
            return;
        };
        let at = self.journal.redo_len() - 1;
        let sound = Sound::of(&undoable.redo, true);
        // Whatever took an undone item's place since is kept, never replaced.
        self.submit(
            undoable.undo,
            Conflict::KeepBoth,
            After::Select,
            Record::Undo(at),
            sound,
            cx,
        );
    }

    fn perform_redo(&mut self, cx: &mut Context<Self>) {
        let Some(undoable) = self.journal.pop_redo() else {
            return;
        };
        let at = self.journal.undo_len() - 1;
        let sound = Sound::of(&undoable.redo, false);
        self.submit(
            undoable.redo,
            undoable.conflict,
            After::Select,
            Record::Redo(at),
            sound,
            cx,
        );
    }

    pub(crate) fn wire_undo_redo(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .on_action(cx.listener(|this, _: &UndoOp, _, cx| this.perform_undo(cx)))
            .on_action(cx.listener(|this, _: &RedoOp, _, cx| this.perform_redo(cx)))
    }
}
