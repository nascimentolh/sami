mod binds;
mod braille;
mod changes;
mod ghostty;
mod herdr;
mod keys;
mod mouse;
mod palette;
mod session;
mod shell;
mod sidebar;
mod terminal;
mod theme;
mod tour;

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
        // a sans do chrome do protótipo vai embutida: não depende de a máquina ter a fonte
        let _ = cx.text_system().add_fonts(vec![
            include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf").into(),
            include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf").into(),
            include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf").into(),
            include_bytes!("../assets/fonts/IBMPlexSans-Bold.ttf").into(),
        ]);
        let config = GhosttyConfig::load();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Maestro".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_decorations: Some(WindowDecorations::Client),
            window_background: if config.background_opacity < 1. {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            let theme = Theme::new(&config.palette, config.font_family.clone());
            let theme_name = if config.palette == palette::Palette::default() {
                "Dracula · "
            } else {
                ""
            };
            let font = format!("{theme_name}{} {}", config.font_family, config.font_size_pt);
            let launch = herdr.as_ref().map(|h| h.launch());
            let binds = if herdr.is_some() {
                binds::Binds::load()
            } else {
                binds::Binds::default()
            };
            let terminal = cx.new(|cx| TerminalView::new(config, launch, binds, cx));
            let anchors = tour::Anchors::default();
            let sidebar =
                herdr.map(|h| cx.new(|cx| Sidebar::new(h, theme.clone(), anchors.clone(), cx)));
            window.focus(&terminal.read(cx).focus_handle(), cx);
            cx.new(|cx| Shell::new(sidebar, terminal, theme, font, anchors, cx))
        })
        .expect("abrir a janela");
    });
}
