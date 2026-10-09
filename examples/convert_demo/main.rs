mod app;
mod drop;
mod jobs;
mod open;
mod thumbs;

use gpui::{App, Bounds, WindowBounds, WindowOptions, actions, point, prelude::*, px, size};
use native_gpui::{MenuCommand, MenuSection, install_menus, prepend_menu, tokens, window_options};

use crate::app::ConvertDemo;

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
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(240.), px(160.)),
                    size: size(px(500.), px(620.)),
                })),
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
