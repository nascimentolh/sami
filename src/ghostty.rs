use std::path::PathBuf;

use alacritty_terminal::vte::ansi::Rgb;

use crate::palette::Palette;

/// O que o Maestro usa do `~/.config/ghostty/config`. Chave ausente fica no default do Ghostty (a paleta, em Dracula).
// ponytail: sem `config-file`/`theme`; entram se a sua config passar a usar
pub struct GhosttyConfig {
    pub font_family: String,
    pub font_size_pt: f32,
    pub font_features: Vec<(String, u32)>,
    pub padding_x: f32,
    pub padding_y: f32,
    pub background_opacity: f32,
    pub palette: Palette,
}

impl Default for GhosttyConfig {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".into(),
            font_size_pt: 13.0,
            font_features: Vec::new(),
            padding_x: 2.0,
            padding_y: 2.0,
            background_opacity: 1.0,
            palette: Palette::default(),
        }
    }
}

impl GhosttyConfig {
    pub fn load() -> Self {
        let path = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("ghostty/config"));
        match path.and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => Self::parse(&text),
            None => Self::default(),
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut c = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            let p = &mut c.palette;
            match key.trim() {
                "font-family" => c.font_family = value.into(),
                "font-size" => c.font_size_pt = value.parse().unwrap_or(c.font_size_pt),
                "font-feature" => c.font_features.push(feature(value)),
                "window-padding-x" => c.padding_x = first_number(value).unwrap_or(c.padding_x),
                "window-padding-y" => c.padding_y = first_number(value).unwrap_or(c.padding_y),
                "background-opacity" => {
                    c.background_opacity =
                        value.parse::<f32>().map(|v| v.clamp(0., 1.)).unwrap_or(1.)
                }
                "background" => set(&mut p.background, value),
                "foreground" => set(&mut p.foreground, value),
                "cursor-color" => set(&mut p.cursor, value),
                "cursor-text" => set(&mut p.cursor_text, value),
                "selection-background" => set(&mut p.selection_bg, value),
                "selection-foreground" => set(&mut p.selection_fg, value),
                "palette" => {
                    if let Some((i, color)) = value.split_once('=')
                        && let Ok(i) = i.trim().parse::<usize>()
                        && i < 16
                    {
                        set(&mut p.ansi[i], color.trim());
                    }
                }
                _ => {}
            }
        }
        c
    }
}

fn set(slot: &mut Rgb, value: &str) {
    if let Some(c) = hex(value) {
        *slot = c;
    }
}

fn hex(value: &str) -> Option<Rgb> {
    let v = value.trim_start_matches('#');
    if v.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some(Rgb {
        r: (n >> 16) as u8,
        g: (n >> 8) as u8,
        b: n as u8,
    })
}

fn first_number(value: &str) -> Option<f32> {
    value.split(',').next()?.trim().parse().ok()
}

/// `calt`, `+calt`, `-calt`, `calt=0` → (tag, valor), como o Ghostty aceita
fn feature(value: &str) -> (String, u32) {
    if let Some(tag) = value.strip_prefix('-') {
        return (tag.into(), 0);
    }
    let value = value.trim_start_matches('+');
    match value.split_once('=') {
        Some((tag, v)) => (tag.trim().into(), v.trim().parse().unwrap_or(1)),
        None => (value.into(), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_a_config_do_ghostty() {
        let c = GhosttyConfig::parse(
            "# comentário\nfont-family = JetBrainsMono Nerd Font\nfont-size = 13\n\
             window-padding-x = 8\nwindow-padding-y = 4,10\nbackground-opacity = 0.95\n\
             background = #282a36\nforeground = #f8f8f2\nselection-background = #44475a\n\
             palette = 1=#ff5555\npalette = 15=#ffffff\npalette = 99=#000000\n\
             font-feature = calt\nfont-feature = -liga\nfont-feature = ss01=2\nkeybind = ctrl+shift+t=new_tab\n",
        );
        assert_eq!(c.font_family, "JetBrainsMono Nerd Font");
        assert_eq!(c.font_size_pt, 13.0);
        assert_eq!((c.padding_x, c.padding_y), (8.0, 4.0));
        assert_eq!(c.background_opacity, 0.95);
        assert_eq!(
            c.palette.ansi[1],
            Rgb {
                r: 0xff,
                g: 0x55,
                b: 0x55
            }
        );
        assert_eq!(
            c.palette.selection_bg,
            Rgb {
                r: 0x44,
                g: 0x47,
                b: 0x5a
            }
        );
        assert_eq!(
            c.font_features,
            vec![("calt".into(), 1), ("liga".into(), 0), ("ss01".into(), 2)]
        );
    }

    #[test]
    fn sem_arquivo_usa_default() {
        let c = GhosttyConfig::parse("");
        assert_eq!(c.font_size_pt, 13.0);
        assert_eq!(c.background_opacity, 1.0);
    }
}
