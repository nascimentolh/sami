use gpui_kit::Keystroke;

/// Binds de split do ghostty/config traduzidos para as teclas do herdr, enviadas ao PTY:
/// o cliente herdr do Maestro trata como se a tecla fosse digitada (o foco é o deste cliente).
#[derive(Default)]
pub struct Binds {
    binds: Vec<(Combo, Vec<u8>)>,
    /// bytes do prefixo do herdr (Ctrl+A no seu config), para a barra mostrar "PREFIX"
    pub prefix: Option<Vec<u8>>,
    /// teclas de cada ação do herdr (`split_vertical`, `new_tab`…), para a paleta
    actions: Vec<(&'static str, Vec<u8>)>,
}

#[derive(Debug, PartialEq)]
struct Combo {
    ctrl: bool,
    shift: bool,
    alt: bool,
    key: String,
}

const HERDR_DEFAULTS: &[(&str, &str)] = &[
    ("prefix", "ctrl+b"),
    ("new_tab", "prefix+c"),
    ("split_vertical", "prefix+v"),
    ("split_horizontal", "prefix+minus"),
    ("focus_pane_left", "prefix+h"),
    ("focus_pane_down", "prefix+j"),
    ("focus_pane_up", "prefix+k"),
    ("focus_pane_right", "prefix+l"),
    ("zoom", "prefix+z"),
];

impl Binds {
    pub fn load() -> Self {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let read = |p: &str| std::fs::read_to_string(home.join(p)).unwrap_or_default();
        Self::parse(
            &read(".config/ghostty/config"),
            &read(".config/herdr/config.toml"),
        )
    }

    pub fn parse(ghostty: &str, herdr: &str) -> Self {
        let herdr_key = |action: &str| -> Option<String> {
            herdr_keys(herdr)
                .find(|(k, _)| *k == action)
                .map(|(_, v)| v.to_string())
                .or_else(|| {
                    HERDR_DEFAULTS
                        .iter()
                        .find(|(k, _)| *k == action)
                        .map(|(_, v)| v.to_string())
                })
        };
        let prefix = herdr_key("prefix").and_then(|p| bytes(&p, &[]));
        let mut binds = Vec::new();
        for line in ghostty.lines() {
            let Some(bind) = line.trim().strip_prefix("keybind") else {
                continue;
            };
            let Some((combo, action)) = bind.trim_start_matches([' ', '=']).split_once('=') else {
                continue;
            };
            let herdr_action = match action.trim() {
                "new_split:down" => "split_horizontal",
                "new_split:right" => "split_vertical",
                "goto_split:left" => "focus_pane_left",
                "goto_split:right" => "focus_pane_right",
                "goto_split:up" => "focus_pane_up",
                "goto_split:down" => "focus_pane_down",
                "toggle_split_zoom" => "zoom",
                "new_tab" => "new_tab",
                _ => continue,
            };
            let (Some(combo), Some(prefix)) = (parse_combo(combo.trim()), &prefix) else {
                continue;
            };
            if let Some(keys) = herdr_key(herdr_action).and_then(|k| bytes(&k, prefix)) {
                binds.push((combo, keys));
            }
        }
        let actions = HERDR_DEFAULTS
            .iter()
            .filter(|(k, _)| *k != "prefix")
            .filter_map(|(k, _)| Some((*k, bytes(&herdr_key(k)?, prefix.as_deref()?)?)))
            .collect();
        Self {
            binds,
            prefix,
            actions,
        }
    }

    pub fn action(&self, name: &str) -> Option<&[u8]> {
        self.actions
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, b)| b.as_slice())
    }

    pub fn get(&self, k: &Keystroke) -> Option<&[u8]> {
        let m = &k.modifiers;
        self.binds
            .iter()
            .find(|(c, _)| {
                c.ctrl == m.control && c.shift == m.shift && c.alt == m.alt && c.key == k.key
            })
            .map(|(_, b)| b.as_slice())
    }
}

/// `nome = "valor"` dentro de `[keys]`; ponytail: sem crate de TOML, o config do herdr é plano
fn herdr_keys(text: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut in_keys = false;
    text.lines().filter_map(move |line| {
        let line = line.split('#').next()?.trim();
        if line.starts_with('[') {
            in_keys = line == "[keys]";
            return None;
        }
        let (k, v) = line.split_once('=')?;
        in_keys.then(|| (k.trim(), v.trim().trim_matches('"')))
    })
}

fn parse_combo(spec: &str) -> Option<Combo> {
    let mut c = Combo {
        ctrl: false,
        shift: false,
        alt: false,
        key: String::new(),
    };
    for part in spec.split('+') {
        match part {
            "ctrl" => c.ctrl = true,
            "shift" => c.shift = true,
            "alt" => c.alt = true,
            key => c.key = named(key)?.to_string(),
        }
    }
    (!c.key.is_empty()).then_some(c)
}

/// Nome de tecla do Ghostty/herdr → texto da tecla (as setas e o enter seguem como nome).
fn named(key: &str) -> Option<&str> {
    Some(match key {
        "minus" => "-",
        "comma" => ",",
        "ampersand" => "&",
        "backslash" => "\\",
        "space" => " ",
        "enter" | "left" | "right" | "up" | "down" => key,
        k if k.chars().count() == 1 => k,
        _ => return None,
    })
}

/// `ctrl+a` → 0x01; `prefix+v` → prefixo + `v`.
fn bytes(spec: &str, prefix: &[u8]) -> Option<Vec<u8>> {
    if let Some(rest) = spec.strip_prefix("prefix+") {
        let mut out = prefix.to_vec();
        out.extend(bytes(rest, &[])?);
        return Some(out);
    }
    match spec.strip_prefix("ctrl+") {
        Some(k) if k.len() == 1 && k.as_bytes()[0].is_ascii_lowercase() => {
            Some(vec![k.as_bytes()[0] - b'a' + 1])
        }
        Some(_) => None,
        None => {
            let k = named(spec)?;
            (k.chars().count() == 1).then(|| k.as_bytes().to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traduz_os_binds_do_ghostty() {
        let ghostty = "keybind = ctrl+shift+enter=new_split:down\nkeybind = ctrl+shift+backslash=new_split:right\n\
                       keybind = ctrl+shift+left=goto_split:left\nkeybind = ctrl+shift+t=new_tab\n\
                       keybind = ctrl+shift+q=quit\n";
        let herdr = "[keys]\nprefix = \"ctrl+a\"\nsplit_vertical = \"prefix+v\" # comentário\n[ui]\nnew_tab = \"x\"\n";
        let b = Binds::parse(ghostty, herdr);
        let k = |s: &str| Keystroke::parse(s).unwrap();
        assert_eq!(b.get(&k("ctrl-shift-enter")), Some(&b"\x01-"[..]));
        assert_eq!(b.get(&k("ctrl-shift-\\")), Some(&b"\x01v"[..]));
        assert_eq!(b.get(&k("ctrl-shift-left")), Some(&b"\x01h"[..]));
        assert_eq!(b.get(&k("ctrl-shift-t")), Some(&b"\x01c"[..]));
        assert_eq!(b.get(&k("ctrl-shift-q")), None);
        assert_eq!(b.get(&k("ctrl-enter")), None);
        assert_eq!(b.action("zoom"), Some(&b"\x01z"[..]));
    }
}
