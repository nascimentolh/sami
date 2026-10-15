use gpui_kit::{Hsla, rgb};

use crate::{palette::Palette, terminal::hsla};

/// Tokens do `:root` do protótipo aprovado. O que existe na paleta do Ghostty vem dela,
/// para o chrome seguir o tema do terminal; o resto são os tons intermediários do protótipo.
#[derive(Clone)]
pub struct Theme {
    pub win: Hsla,
    pub panel: Hsla,
    pub raise: Hsla,
    pub line: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub comment: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub ok: Hsla,
    pub cyan: Hsla,
    pub orange: Hsla,
    pub sans: &'static str,
    pub mono: String,
}

impl Theme {
    pub fn new(p: &Palette, mono: String) -> Self {
        let accent = hsla(p.ansi[4]);
        Self {
            win: hsla(p.background),
            panel: hsla(p.ansi[0]),
            raise: rgb(0x313342).into(),
            line: rgb(0x3b3d50).into(),
            fg: hsla(p.foreground),
            muted: rgb(0x9a9dbb).into(),
            comment: hsla(p.ansi[8]),
            accent,
            accent_soft: Hsla { a: 0.16, ..accent },
            ok: hsla(p.ansi[2]),
            cyan: hsla(p.ansi[6]),
            orange: rgb(0xffb86c).into(),
            sans: "IBM Plex Sans",
            mono,
        }
    }
}
