mod binds;
mod braille;
mod ghostty;
mod herdr;
mod keys;
mod mouse;
mod palette;
mod shell;
mod sidebar;
mod terminal;
mod theme;

use std::sync::Arc;

use ghostty::GhosttyConfig;
use gpui_kit::*;
use herdr::Herdr;
use shell::Shell;
use sidebar::Sidebar;
use terminal::TerminalView;
use theme::Theme;

fn main() {
    // aberto de dentro de um pane do herdr, o herdr do Maestro se recusaria a rodar ("nested herdr")
    for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("HERDR_")) {
        // SAFETY: ainda não há outras threads
        unsafe { std::env::remove_var(key) };
    }
    let herdr = Herdr::from_env().map(Arc::new);
    gpui_kit::application().run(move |cx| {
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
            let theme = Theme::new(&config.palette, config.font_family.clone());
            let font = format!("{} {}", config.font_family, config.font_size_pt);
            let command = herdr.as_ref().map(|h| h.command());
            let binds = if herdr.is_some() {
                binds::Binds::load()
            } else {
                binds::Binds::default()
            };
            let terminal = cx.new(|cx| TerminalView::new(config, command, binds, cx));
            let sidebar = herdr.map(|h| cx.new(|cx| Sidebar::new(h, theme.clone(), cx)));
            window.focus(&terminal.read(cx).focus_handle(), cx);
            cx.new(|cx| Shell::new(sidebar, terminal, theme, font, cx))
        })
        .expect("abrir a janela");
    });
}
