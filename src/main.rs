use std::time::Instant;

use gpui_kit::*;

const BG: u32 = 0x282a36;
const FG: u32 = 0xf8f8f2;

struct Maestro {
    started: Instant,
    first_frame: bool,
}

impl Render for Maestro {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.first_frame {
            self.first_frame = false;
            if std::env::var_os("MAESTRO_STARTUP").is_some() {
                eprintln!("primeiro frame em {:?}", self.started.elapsed());
            }
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child("maestro")
    }
}

fn main() {
    let started = Instant::now();
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Maestro".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, move |_, cx| {
            cx.new(|_| Maestro {
                started,
                first_frame: true,
            })
        })
        .expect("abrir a janela");
    });
}
