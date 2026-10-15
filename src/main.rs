mod ghostty;
mod keys;
mod mouse;
mod palette;
mod terminal;

use ghostty::GhosttyConfig;
use gpui_kit::*;
use terminal::TerminalView;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        let config = GhosttyConfig::load();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Maestro".into()),
                ..Default::default()
            }),
            window_background: if config.background_opacity < 1. {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| TerminalView::new(config, cx));
            window.focus(&view.read(cx).focus_handle(), cx);
            view
        })
        .expect("abrir a janela");
    });
}
