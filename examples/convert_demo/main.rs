//! Compact local converter: images and audio, copy / remux / encode, then a size report.

mod app;
mod drop;
mod jobs;
mod open;
mod thumbs;

use gpui::{App, WindowOptions, actions, prelude::*, px, size};
use native_gpui::{MenuCommand, MenuSection, install_menus, prepend_menu, tokens, window_options};

use crate::app::{ConvertDemo, window_bounds};

actions!(convert_demo, [OpenFiles, ConvertNow]);

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        prepend_menu(
            MenuSection::File,
            [
                MenuCommand::new("Open…", OpenFiles).key("cmd-o"),
                MenuCommand::new("Convert", ConvertNow).key("cmd-return"),
            ],
        );
        cx.open_window(
            WindowOptions {
                window_bounds: Some(window_bounds()),
                titlebar: None,
                is_minimizable: false,
                is_resizable: true,
                window_min_size: Some(size(px(420.), px(520.))),
                ..window_options()
            },
            |_, cx| cx.new(ConvertDemo::new),
        )
        .unwrap();
        cx.activate(true);
    });
}
