//! The preview pane: Finder's inspector beside the file list.
//!
//! Owns the pane's surface, its drag-resizable width, and the info card — which the column view's
//! trailing preview column draws too, so the card takes a content width and has no width, no
//! background and no insets of its own beyond the one text inset every band shares.
//!
//! Deliberately not here: the gallery view's large preview, which is a different thing in a
//! different place, and where the pane sits in the window, which is `main`'s row.

use gpui::{
    AnyElement, Context, ElementId, FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, SharedString, Window, div, prelude::*, px,
};
use native_gpui::tokens::{self, SymbolColor, SystemColor, sf_symbol_colored, system_color};
use native_gpui::{Cursor, Material, NativeMaterial, cursor_region, material};

use crate::FinderDemo;
use crate::data::{Face, Line, Row};
use crate::palette::Palette;
use crate::ui::{TAGS, artwork, tag_dot};

/// Where the pane starts: 239pt of content plus the 1pt divider on its leading edge, which is what
/// leaves Finder 594pt of list in a 1026pt window. Get it wrong and every column lands somewhere
/// else.
///
/// Not a constant to match, though. Finder persists the width per window, so the same build
/// measured a 239pt pane in the owner's window and a 311pt one in another opened beside it, and
/// matching either would only mean matching that window. This is where the pane starts, and
/// [`FinderDemo::preview_resize`] moves it.
pub(crate) const PREVIEW_W: f32 = 240.;

/// The pane's own minimum, and a hard stop. One more point of drag past it does not narrow the
/// pane further, it collapses it outright.
const MIN_W: f32 = 240.;
/// The pane's own maximum, and the constraint that actually binds. It held at 480pt at window
/// widths 1226, 1026, 988 and 748, so it does not move with the window: the travel stops against
/// the pane, not only against the list.
const MAX_W: f32 = 480.;
/// What the file list keeps, also hard. Finder's *window* minimum is derived from it: with the pane
/// at 240 the window will not go below 748 and with the pane at 480 it will not go below 988, and
/// both of those are 192 + 316 + the pane. Two window minima, one list minimum — see
/// [`FinderDemo::sync_window_min`], which is where the derivation is applied.
pub(crate) const LIST_MIN_W: f32 = 316.;

// The pane never crosses the list, however far the divider travels. The list's trailing edge and
// the pane's leading edge stayed the same number at every divider position at window widths 1226,
// 1026, 988 and 748 — and the same subtraction reported 121pt for the list's own horizontal clip
// in the same reading, so that null is "they do not overlap" and not "this cannot see overlap".
//
// What reads as the pane sliding over the list is the list cutting its own columns. Finder's list
// columns are fixed and its Name column floors at 168pt, so the row content stays a constant 589pt
// wide however narrow the pane leaves the list: 121pt of it is cut at a 468pt list and 273pt at
// the 316pt minimum. That is what cuts a Kind string mid-word.
//
// The pane's material does sample the list, which is the other half of the same impression. With
// rows selected, the pane's leading 84pt carried a blurred copy of the selection band at the same
// y to within 3pt — 12 units against the list's own 55 — while the pane's trailing strip, which
// has no list behind it, stayed flat through that y and moved 27x less. 84 is a floor, not the
// bleed's extent: the card's artwork is opaque and covers the surface beyond it.

/// The grab strip on the pane's leading edge.
const GRIP_W: f32 = 6.;

/// One inset for the whole text block, both edges. Every band in Finder's pane starts 9.5–10.0pt
/// from the pane's leading edge — filename, kind, Information, Tags, Add Tags… — and the values,
/// the Show More link and the row hairlines all end 10.0–11.0pt in from the trailing edge.
const INSET: f32 = 10.;

/// The filename's size. Measured, not chosen: the two captured names inked 88.5 and 59.5pt, and a
/// pixel ink box runs about a point wider than CoreText's, so the ink *difference* of 29.0pt is the
/// threshold-free statistic. 15pt reproduces it at 29.35 where 14pt gives 27.4 and 16pt gives 31.3.
/// Weight is not separable at this precision; semibold is what AppKit emphasises a title with.
const NAME_SIZE: f32 = 15.;

// The pane's vertical model, measured on two Finder windows of different pane WIDTH and three of
// different window HEIGHT, all at 2x in dark mode:
//
//   * The artwork sits in a SQUARE whose side is the content width, top-anchored 45pt below the
//     toolbar band. A 291pt-wide pane put its square at y 96.5 and a 240pt one at 97.5, and every
//     artwork — a 16:9 movie thumbnail, a 4:3 image, a folder icon — was aspect-fit and centred in
//     it, in both windows, to within 1pt.
//   * The text block hangs off the square's bottom, so it MOVES with the pane's width: narrowing
//     the pane from 311 to 240 moved every band from the filename to Add Tags… up by exactly
//     70.5pt, and left their spacing untouched.
//   * The action row is pinned to the WINDOW's bottom and moves with nothing else. Across window
//     heights of 620, 772 and 900 its icons ink 39.0pt from the bottom and its labels 18.0pt, to
//     0.0pt; across the same pane-width change it did not move either.
//   * Everything above the action row is independent of window HEIGHT: at 772 and 900 the bands
//     are identical to 0.0pt. The gap above the action row is what absorbs the difference.
//
// So the deltas below are gaps between line boxes, exact by construction, and only two numbers
// place the whole thing. Finder's ink tops in a 240pt pane: filename 382.5, kind 404.0,
// Information 427.5, rows 451.5 / 474.5 / 497.5, Tags 529.5, Add Tags… 551.0, action icons 718.0,
// action labels 745.5.
const NAME_H: f32 = 18.;
const LINE_H: f32 = 15.;
const SMALL_H: f32 = 13.;
const GAP_NAME_KIND: f32 = 4.;
const GAP_KIND_HEAD: f32 = 9.;
const GAP_HEAD_ROWS: f32 = 8.5;
/// 8 between 15pt boxes is the measured 23pt row pitch.
const ROW_GAP: f32 = 8.;
const GAP_ROWS_TAGS: f32 = 17.;
const GAP_TAGS_ADD: f32 = 6.5;
const ICON_H: f32 = 16.;
const GAP_ICON_LABEL: f32 = 10.5;
const BOTTOM_INSET: f32 = 16.;
/// Toolbar band to the top of the artwork square.
const GAP_TOP: f32 = 45.;
/// Square's bottom to the filename's box. Its ink sat 66.0pt below the square in a 240pt pane and
/// 65.5 in a 311pt one.
const GAP_SQUARE_TEXT: f32 = 62.;

/// Finder's action buttons sit in equal slots, not at equal gaps: the three icon centres measured
/// 795.0 / 869.8 / 946.0, a 75.5pt pitch, while the gaps between their content boxes were 35.5 and
/// 47.0. The middle slot's centre is the pane's centre.
const ACTION_SLOT_W: f32 = 75.5;
/// The point size whose ink is the measured 14.0pt tall.
const ACTION_ICON_SIZE: f32 = 15.;

/// The pane's surface.
///
/// [`NativeMaterial::dims_when_inactive`] is the non-default here and it is the whole point. The
/// crate pins `state = Active` so a sidebar cannot collapse to grey behind another window, but
/// Finder's preview pane measurably does exactly that: 27 key, 46 non-key in dark mode, against a
/// file list that is byte-identical across the same pair of captures. A flat colour cannot move,
/// so that delta is the only thing that shows this surface is a material and not a fill — and the
/// pane and the sidebar part company there (46 against 41), so they are two views, not one.
///
/// `Sidebar`, not `WindowBackground`: measured on our own window, `WindowBackground` renders 67 in
/// dark against Finder's 27 — the same 67 the sidebar's backdrop comment records — and it is the
/// material's own tint, not the desktop showing through, because a window-scoped capture cannot
/// see a material sampling the desktop.
pub(crate) fn preview_material(id: impl Into<ElementId>) -> NativeMaterial {
    material(id, Material::Sidebar).dims_when_inactive(true)
}

impl FinderDemo {
    /// The whole pane: surface, divider, drag grip and card.
    ///
    /// `top` is the toolbar band, applied to the *card* rather than to the pane, because the
    /// surface has to run up behind the toolbar. Finder's has no top edge at all — at x 960 and
    /// x 1010 the grey is a constant 27 from y 42 straight through y 54, with no step where the
    /// toolbar ends — while the file list's column-header band and its hairline stop dead at the
    /// pane's leading edge.
    pub(crate) fn preview_pane(
        &self,
        line: Option<&Line>,
        palette: &Palette,
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = self.preview_w - 1. - 2. * INSET;
        preview_material("finder.preview")
            .w(px(self.preview_w))
            .h_full()
            .flex_none()
            .relative()
            // The divider is the material's own border, which is also this pane's standing proof
            // that a material paints its GPUI style at all: it did not until this pass, and the
            // only symptom of that was a missing line.
            .border_l_1()
            .border_color(palette.separator)
            .child(
                div()
                    .size_full()
                    .pt(top)
                    .child(self.info_card(line, palette, content, cx)),
            )
            .child(cursor_region(
                "finder.preview.grip",
                Cursor::ColumnResize,
                div()
                    .absolute()
                    .left_0()
                    .top(top)
                    .bottom_0()
                    .w(px(GRIP_W))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.preview_drag =
                                Some(f32::from(event.position.x) + this.preview_w);
                            cx.notify();
                        }),
                    ),
            ))
            .into_any_element()
    }

    /// The card, on whatever surface the caller has: artwork, name, kind, the Information block,
    /// Tags, and the action row. `content` is the width available *inside* the insets, which sizes
    /// the artwork; the card has no width of its own and fills its parent's height.
    pub(crate) fn info_card(
        &self,
        line: Option<&Line>,
        palette: &Palette,
        content: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // With nothing selected Finder's pane is the bare surface and carries no caption, so an
        // empty div is the match rather than a "No selection" label.
        let Some(line) = line else {
            return div().into_any_element();
        };
        let row = &line.row;
        let body = tokens::system_font_size();
        let small = tokens::small_system_font_size();
        let hairline = system_color(SystemColor::QuaternarySystemFill);
        let rows = info_rows(row, self.preview_expanded);
        let media = row.dimensions.is_some() || row.duration.is_some();

        let card = div()
            .size_full()
            .flex()
            .flex_col()
            .px(px(INSET))
            .pb(px(BOTTOM_INSET))
            .child(div().h(px(GAP_TOP)).flex_none())
            // The artwork square: side = the content width, top-anchored, artwork aspect-fit and
            // centred inside it. Nothing here flexes, which is the measured behaviour — at window
            // heights of 772 and 900 every band above the action row is identical to 0.0pt.
            .child(
                div()
                    .w(px(content))
                    .h(px(content))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .child(hero(row, content)),
            );

        let mut lower = div()
            .flex_none()
            .flex()
            .flex_col()
            .mt(px(GAP_SQUARE_TEXT))
            .child(
                div()
                    .line_height(px(NAME_H))
                    .text_size(px(NAME_SIZE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(row.name.clone()),
            )
            .child(
                div()
                    .mt(px(GAP_NAME_KIND))
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .text_color(palette.secondary)
                    .child(kind_line(row)),
            )
            .child(
                div()
                    .mt(px(GAP_KIND_HEAD))
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .child("Information")
                    // Present for the owner's movie and for a PNG, absent for a movie carrying no
                    // frame metadata: the link appears when there is something left to reveal.
                    .when(media, |heading| {
                        heading.child(
                            div()
                                .line_height(px(SMALL_H))
                                .text_size(small)
                                .font_weight(FontWeight::NORMAL)
                                .text_color(system_color(SystemColor::Link))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                        this.preview_expanded = !this.preview_expanded;
                                        cx.notify();
                                    }),
                                )
                                .child(if self.preview_expanded {
                                    "Show Less"
                                } else {
                                    "Show More"
                                }),
                        )
                    }),
            );

        let mut block = div().mt(px(GAP_HEAD_ROWS)).flex().flex_col();
        for (index, (label, value)) in rows.iter().enumerate() {
            if index > 0 {
                // Two hairlines for three rows: between them only, none above the first or below
                // the last, and inset to the text block rather than run full-bleed.
                block = block.child(
                    div()
                        .h(px(ROW_GAP))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(div().h(px(1.)).bg(hairline)),
                );
            }
            block = block.child(
                div()
                    .flex()
                    .justify_between()
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .child(div().text_color(palette.secondary).child(*label))
                    // The value is the brighter of the pair — labelColor against the label's
                    // secondaryLabelColor, measured 232 against 153 — which is the opposite way
                    // round from the obvious choice.
                    .child(div().text_color(palette.label).child(value.clone())),
            );
        }
        lower = lower.child(block).child(
            div()
                .mt(px(GAP_ROWS_TAGS))
                .line_height(px(LINE_H))
                .text_size(body)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.label)
                .child("Tags"),
        );

        lower = match self.tags.get(&line.key) {
            Some(tag) => lower.child(
                div()
                    .mt(px(GAP_TAGS_ADD))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .child(tag_dot(*tag))
                    .child(div().text_color(palette.label).child(TAGS[*tag].0)),
            ),
            // A third tier, not secondary: 94 against the row labels' 153.
            None => lower.child(
                div()
                    .mt(px(GAP_TAGS_ADD))
                    .line_height(px(LINE_H))
                    .text_size(body)
                    .text_color(palette.tertiary)
                    .child("Add Tags…"),
            ),
        };

        let actions = actions(row.face);
        let slot = ACTION_SLOT_W.min(content / actions.len() as f32);
        card.child(lower)
            // The gap above the action row is what absorbs a height change: across window heights
            // of 620, 772 and 900 the action row's icons inked 39.0pt from the window's bottom and
            // its labels 18.0pt, to 0.0pt, while every band above it stayed put.
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .children(actions.iter().map(|(symbol, label)| {
                        div()
                            .w(px(slot))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div().h(px(ICON_H)).flex().items_center().children(
                                    sf_symbol_colored(
                                        symbol,
                                        px(ACTION_ICON_SIZE),
                                        FontWeight::NORMAL,
                                        &SymbolColor::Hierarchical(SystemColor::SecondaryLabel),
                                    )
                                    .map(|glyph| glyph.img()),
                                ),
                            )
                            .child(
                                div()
                                    .mt(px(GAP_ICON_LABEL))
                                    .line_height(px(SMALL_H))
                                    .text_size(small)
                                    .text_color(palette.secondary)
                                    .child(*label),
                            )
                    })),
            )
            .into_any_element()
    }

    /// Widen or narrow the pane by dragging its divider.
    ///
    /// Both ends of the travel are the pane's own — 240pt and 480pt — and the 480 held at four
    /// window widths, so the earlier reading that the pane has no maximum of its own and only the
    /// list gives way was measuring a window too wide for the maximum to bind. The list's 316pt
    /// minimum is a second, independent stop, and it only binds once the window is narrow enough
    /// for the two to meet.
    pub(crate) fn preview_resize(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self.preview_drag else {
            return;
        };
        // A mouse-up outside the window never reaches the root's handler, so the button state on
        // the next move back inside it is what ends the drag.
        if !event.dragging() {
            self.preview_drag = None;
            return;
        }
        let metrics = self.metrics();
        let room = f32::from(window.viewport_size().width)
            - f32::from(metrics.left)
            - f32::from(metrics.right);
        let width = (anchor - f32::from(event.position.x))
            .clamp(MIN_W, MAX_W.min(room - LIST_MIN_W).max(MIN_W));
        if width != self.preview_w {
            self.preview_w = width;
            cx.notify();
        }
    }

    pub(crate) fn preview_resize_end(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_drag.take().is_some() {
            cx.notify();
        }
    }
}

/// The artwork inside the square. A movie or an image gets the thumbnail's *geometry* — the full
/// content width, aspect-fit, rounded corners, which is what Finder's 219 x 123 thumbnail of a
/// 640x360 source is in a 240pt pane — with the type's own artwork on it, because nothing in this
/// demo has pixels to thumbnail. Everything else gets the system artwork at the square's own size.
///
/// No inset: Finder's folder ink measured 0.7406 and 0.7420 of the square in two panes of
/// different width, and our own `NSWorkspace` rendition inks 0.7695 of what it is drawn in. That
/// 3.7% is either a 5pt inset or a slightly different rendition, and two windows cannot tell them
/// apart, so this asks for the square and reports the residual rather than fitting a constant.
fn hero(row: &Row, content: f32) -> AnyElement {
    if matches!(row.face, Face::Movie | Face::Image) {
        return div()
            .w(px(content))
            .h(px(content * 9. / 16.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .bg(system_color(SystemColor::TertiarySystemFill))
            .child(artwork(row, (content / 3.).min(96.)))
            .into_any_element();
    }
    artwork(row, content)
}

/// `Kind - Size`, joined by a plain hyphen: Finder's own line inked 152.5pt for
/// `QuickTime movie - 94 KB`, which 13pt regular with a hyphen reproduces to within 0.8pt where an
/// en dash misses by 3.6 and an em dash by 7.3. An item with no computed size — every folder here
/// — is the kind alone, which is what the owner's folder preview shows.
fn kind_line(row: &Row) -> SharedString {
    match row.size.as_ref() {
        "--" => row.face.kind().into(),
        size => format!("{} - {size}", row.face.kind()).into(),
    }
}

/// Finder's Information rows are driven by the metadata the item carries rather than being a fixed
/// list: the owner's folder shows Modified and Size, their movie shows Created, Modified,
/// Dimensions and Duration. Show More is what reveals the rest.
fn info_rows(row: &Row, expanded: bool) -> Vec<(&'static str, SharedString)> {
    let media = row.dimensions.is_some() || row.duration.is_some();
    let mut rows = Vec::with_capacity(6);
    if media {
        rows.push(("Created", row.created.clone()));
    }
    rows.push(("Modified", row.modified.clone()));
    if media {
        rows.extend(row.dimensions.clone().map(|value| ("Dimensions", value)));
        rows.extend(row.duration.clone().map(|value| ("Duration", value)));
        if expanded {
            rows.push(("Kind", row.face.kind().into()));
            rows.push(("Size", row.size.clone()));
        }
    } else {
        rows.push(("Size", row.size.clone()));
    }
    rows
}

/// Type-dependent, and measured: a folder gets `More…` alone and a movie gets three. Bare glyphs
/// over the pane with no button plate — a tight box round Finder's first icon reads the pane's own
/// background at 68 distinct greys. The image row is inferred from the movie's, not measured.
fn actions(face: Face) -> &'static [(&'static str, &'static str)] {
    match face {
        Face::Movie => &[
            ("rotate.left", "Rotate Left"),
            ("timeline.selection", "Trim"),
            ("ellipsis.circle", "More…"),
        ],
        Face::Image => &[("rotate.left", "Rotate Left"), ("ellipsis.circle", "More…")],
        _ => &[("ellipsis.circle", "More…")],
    }
}
