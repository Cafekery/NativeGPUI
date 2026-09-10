use finder_fs::kind::Category;
use gpui::{AnyElement, Context, div, prelude::*, px};
use native_gpui::tokens::{self, SystemColor};
use native_gpui::{
    ButtonBezel, ControlSize, native_button, native_label, native_popup, native_text_field,
};

use crate::app::FinderDemo;
use crate::data::Row;

pub(crate) const CRITERIA_H: f32 = 34.;
const INSET: f32 = 8.;
const GAP: f32 = 6.;
const IS_W: f32 = 14.;
const BUTTON_W: f32 = 33.;
const OTHER_W: f32 = 127.;

const FIELDS: [&str; 2] = ["Kind", "Name"];
const TESTS: [&str; 4] = ["contains", "begins with", "ends with", "is"];

/// One row of Finder's search criteria: what to test, and what it has to be.
#[derive(Clone, Default)]
pub(crate) struct Criterion {
    pub(crate) field: usize,
    pub(crate) kind: Category,
    pub(crate) test: usize,
    pub(crate) text: String,
}

impl Criterion {
    pub(crate) fn holds(&self, row: &Row) -> bool {
        match FIELDS[self.field.min(FIELDS.len() - 1)] {
            "Name" => {
                let name = row.name.to_lowercase();
                let wanted = self.text.to_lowercase();
                match TESTS[self.test.min(TESTS.len() - 1)] {
                    "begins with" => name.starts_with(&wanted),
                    "ends with" => name.ends_with(&wanted),
                    "is" => name == wanted,
                    _ => name.contains(&wanted),
                }
            }
            // Finder's kinds overlap, so this asks what the type conforms to rather than sorting
            // each file into one of them.
            _ => match self.kind {
                Category::Other => row.kind.to_lowercase().contains(&self.text.to_lowercase()),
                kind => finder_fs::kind::conforms(&row.uti, kind),
            },
        }
    }

    fn names(&self) -> bool {
        FIELDS[self.field.min(FIELDS.len() - 1)] == "Name"
    }
}

impl FinderDemo {
    pub(crate) fn criteria_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .children(
                (0..self.criteria.len())
                    .map(|row| self.criteria_row(row, cx))
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }

    fn criteria_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
        let criterion = &self.criteria[row];
        let value = Category::ALL
            .iter()
            .position(|it| *it == criterion.kind)
            .unwrap_or(0);
        let (field, pick, test, edit, add, remove) = (
            cx.entity().downgrade(),
            cx.entity().downgrade(),
            cx.entity().downgrade(),
            cx.entity().downgrade(),
            cx.entity().downgrade(),
            cx.entity().downgrade(),
        );
        let names = criterion.names();
        div()
            .h(px(CRITERIA_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .pl(px(INSET))
            .pr(px(INSET))
            .child(
                native_popup(("ng.finder.criteria.field", row), criterion.field)
                    .items(FIELDS)
                    .control_size(ControlSize::Small)
                    .on_change(move |index, _, cx| {
                        let index = *index;
                        field
                            .update(cx, |this, cx| this.set_criterion_field(row, index, cx))
                            .ok();
                    }),
            )
            .children((!names).then(|| {
                div().flex_none().w(px(IS_W)).ml(px(GAP)).child(
                    native_label(("ng.finder.criteria.is", row), "is")
                        .font_size(tokens::small_system_font_size())
                        .color(SystemColor::SecondaryLabel),
                )
            }))
            .children(names.then(|| {
                div().flex_none().ml(px(GAP)).child(
                    native_popup(("ng.finder.criteria.test", row), criterion.test)
                        .items(TESTS)
                        .control_size(ControlSize::Small)
                        .on_change(move |index, _, cx| {
                            let index = *index;
                            test.update(cx, |this, cx| this.set_criterion_test(row, index, cx))
                                .ok();
                        }),
                )
            }))
            .children((!names).then(|| {
                div().flex_none().ml(px(GAP)).child(
                    native_popup(("ng.finder.criteria.kind", row), value)
                        .items(Category::ALL.map(|kind| kind.label()))
                        .control_size(ControlSize::Small)
                        .on_change(move |index, _, cx| {
                            let kind = Category::ALL[(*index).min(Category::ALL.len() - 1)];
                            pick.update(cx, |this, cx| this.set_criterion(row, kind, cx))
                                .ok();
                        }),
                )
            }))
            .children((names || criterion.kind == Category::Other).then(|| {
                div().flex_none().w(px(OTHER_W)).ml(px(GAP)).child(
                    native_text_field(("ng.finder.criteria.other", row), &criterion.text)
                        .control_size(ControlSize::Small)
                        .on_change(move |text: &String, _, cx| {
                            let text = text.clone();
                            edit.update(cx, |this, cx| this.set_criterion_text(row, text, cx))
                                .ok();
                        }),
                )
            }))
            .child(div().flex_1())
            .child(
                div().flex_none().w(px(BUTTON_W)).child(
                    native_button(("ng.finder.criteria.remove", row), "\u{2212}")
                        .bezel_style(ButtonBezel::AccessoryBarAction)
                        .control_size(ControlSize::Small)
                        .enabled(self.criteria.len() > 1)
                        .on_click(move |_, _, cx| {
                            remove
                                .update(cx, |this, cx| this.drop_criterion(row, cx))
                                .ok();
                        }),
                ),
            )
            .child(
                div().flex_none().w(px(BUTTON_W)).child(
                    native_button(("ng.finder.criteria.add", row), "+")
                        .bezel_style(ButtonBezel::AccessoryBarAction)
                        .control_size(ControlSize::Small)
                        .on_click(move |_, _, cx| {
                            add.update(cx, |this, cx| this.add_criterion(cx)).ok();
                        }),
                ),
            )
            .into_any_element()
    }

    fn set_criterion_field(&mut self, row: usize, field: usize, cx: &mut Context<Self>) {
        if let Some(criterion) = self.criteria.get_mut(row) {
            criterion.field = field;
        }
        self.run_search(cx);
    }

    fn set_criterion_test(&mut self, row: usize, test: usize, cx: &mut Context<Self>) {
        if let Some(criterion) = self.criteria.get_mut(row) {
            criterion.test = test;
        }
        self.run_search(cx);
    }

    fn set_criterion(&mut self, row: usize, kind: Category, cx: &mut Context<Self>) {
        if let Some(criterion) = self.criteria.get_mut(row) {
            criterion.kind = kind;
        }
        self.run_search(cx);
    }

    fn set_criterion_text(&mut self, row: usize, text: String, cx: &mut Context<Self>) {
        if let Some(criterion) = self.criteria.get_mut(row) {
            criterion.text = text;
        }
        self.run_search(cx);
    }

    fn add_criterion(&mut self, cx: &mut Context<Self>) {
        self.criteria.push(Criterion::default());
        self.run_search(cx);
    }

    fn drop_criterion(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.criteria.len() > 1 {
            self.criteria.remove(row);
        }
        self.run_search(cx);
    }
}
