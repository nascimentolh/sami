mod keys;
mod palette;
mod terminal;

use gpui_kit::*;
use terminal::TerminalView;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Maestro".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            let view = cx.new(TerminalView::new);
            window.focus(&view.read(cx).focus_handle(), cx);
            view
        })
        .expect("abrir a janela");
    });
}
