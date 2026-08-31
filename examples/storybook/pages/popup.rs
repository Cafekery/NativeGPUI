use crate::Demo;
use crate::ui::*;
use gpui::{AnyElement, Context, Window};

#[derive(Default)]
pub struct State;

pub fn render(_d: &mut Demo, _window: &mut Window, _cx: &mut Context<Demo>) -> AnyElement {
    stack(vec![note("This page has not been built yet.")])
}
