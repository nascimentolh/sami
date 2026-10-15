use gpui_kit::Keystroke;

pub fn to_bytes(k: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    let arrow = |c: u8| {
        Some(if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        })
    };
    let named: Option<Vec<u8>> = match k.key.as_str() {
        "enter" => Some(b"\r".to_vec()),
        "backspace" => Some(if m.alt {
            b"\x1b\x7f".to_vec()
        } else {
            b"\x7f".to_vec()
        }),
        "tab" => Some(if m.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(b"\x1b".to_vec()),
        "up" => arrow(b'A'),
        "down" => arrow(b'B'),
        "right" => arrow(b'C'),
        "left" => arrow(b'D'),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "space" if m.control => Some(vec![0]),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if m.control && !m.alt {
        let c = k.key.chars().next().filter(|_| k.key.len() == 1)?;
        let byte = match c {
            'a'..='z' => c as u8 - b'a' + 1,
            '[' => 0x1b,
            '\\' => 0x1c,
            ']' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' => 0x1f,
            _ => return None,
        };
        return Some(vec![byte]);
    }
    let text = k.key_char.as_ref()?;
    let mut bytes = Vec::with_capacity(text.len() + 1);
    if m.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn mapeia_teclas() {
        assert_eq!(to_bytes(&ks("enter"), false), Some(b"\r".to_vec()));
        assert_eq!(to_bytes(&ks("ctrl-c"), false), Some(vec![3]));
        assert_eq!(to_bytes(&ks("ctrl-a"), false), Some(vec![1]));
        assert_eq!(to_bytes(&ks("up"), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(to_bytes(&ks("up"), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(to_bytes(&ks("shift-tab"), false), Some(b"\x1b[Z".to_vec()));
    }
}
