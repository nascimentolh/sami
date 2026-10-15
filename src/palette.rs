use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

#[derive(Clone, PartialEq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection_bg: Rgb,
    pub selection_fg: Rgb,
}

const fn rgb(hex: u32) -> Rgb {
    Rgb {
        r: (hex >> 16) as u8,
        g: (hex >> 8) as u8,
        b: hex as u8,
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            ansi: [
                rgb(0x21222c),
                rgb(0xff5555),
                rgb(0x50fa7b),
                rgb(0xf1fa8c),
                rgb(0xbd93f9),
                rgb(0xff79c6),
                rgb(0x8be9fd),
                rgb(0xf8f8f2),
                rgb(0x6272a4),
                rgb(0xff6e6e),
                rgb(0x69ff94),
                rgb(0xffffa5),
                rgb(0xd6acff),
                rgb(0xff92df),
                rgb(0xa4ffff),
                rgb(0xffffff),
            ],
            foreground: rgb(0xf8f8f2),
            background: rgb(0x282a36),
            cursor: rgb(0xf8f8f2),
            cursor_text: rgb(0x282a36),
            selection_bg: rgb(0x44475a),
            selection_fg: rgb(0xffffff),
        }
    }
}

impl Palette {
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(c) => c,
            Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| self.indexed(i)),
            Color::Named(n) => overrides[n as usize].unwrap_or_else(|| self.named(n)),
        }
    }

    fn named(&self, n: NamedColor) -> Rgb {
        use NamedColor::*;
        match n {
            Foreground | BrightForeground => self.foreground,
            Background => self.background,
            Cursor => self.cursor,
            DimForeground => dim(self.foreground),
            DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan
            | DimWhite => dim(self.ansi[n as usize - DimBlack as usize]),
            _ => self.ansi[n as usize],
        }
    }

    fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..=15 => self.ansi[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                Rgb {
                    r: level(i / 36),
                    g: level(i / 6 % 6),
                    b: level(i % 6),
                }
            }
            _ => {
                let v = (i - 232) * 10 + 8;
                Rgb { r: v, g: v, b: v }
            }
        }
    }
}

fn dim(c: Rgb) -> Rgb {
    Rgb {
        r: (c.r as u16 * 2 / 3) as u8,
        g: (c.g as u16 * 2 / 3) as u8,
        b: (c.b as u16 * 2 / 3) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xterm_256() {
        let p = Palette::default();
        let none = Colors::default();
        assert_eq!(p.resolve(Color::Indexed(1), &none), rgb(0xff5555));
        assert_eq!(p.resolve(Color::Indexed(16), &none), rgb(0x000000));
        assert_eq!(p.resolve(Color::Indexed(196), &none), rgb(0xff0000));
        assert_eq!(p.resolve(Color::Indexed(231), &none), rgb(0xffffff));
        assert_eq!(p.resolve(Color::Indexed(232), &none), rgb(0x080808));
        assert_eq!(p.resolve(Color::Indexed(255), &none), rgb(0xeeeeee));
        assert_eq!(
            p.resolve(Color::Named(NamedColor::Background), &none),
            rgb(0x282a36)
        );
    }
}
