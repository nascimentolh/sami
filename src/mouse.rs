use gpui_kit::Modifiers;

pub const LEFT: u8 = 0;
pub const MIDDLE: u8 = 1;
pub const RIGHT: u8 = 2;
pub const NO_BUTTON: u8 = 3;
pub const WHEEL_UP: u8 = 64;
pub const WHEEL_DOWN: u8 = 65;
const DRAG: u8 = 32;

/// Relato de mouse para o app do terminal (vim, btop, herdr). `col`/`line` começam em 0.
pub fn report(
    button: u8,
    drag: bool,
    pressed: bool,
    col: usize,
    line: usize,
    mods: &Modifiers,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mut b = button + if drag { DRAG } else { 0 };
    if mods.shift {
        b += 4;
    }
    if mods.alt {
        b += 8;
    }
    if mods.control {
        b += 16;
    }
    if sgr {
        let end = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{};{};{}{}", b, col + 1, line + 1, end).into_bytes());
    }
    // modo X10/normal: soltar é sempre o botão 3, e a posição cabe em um byte
    if col > 222 || line > 222 {
        return None;
    }
    let b = if pressed { b } else { 3 + (b & !3) };
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + b,
        33 + col as u8,
        33 + line as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_e_x10() {
        let none = Modifiers::default();
        assert_eq!(
            report(LEFT, false, true, 0, 0, &none, true).unwrap(),
            b"\x1b[<0;1;1M"
        );
        assert_eq!(
            report(LEFT, false, false, 9, 4, &none, true).unwrap(),
            b"\x1b[<0;10;5m"
        );
        assert_eq!(
            report(LEFT, true, true, 2, 3, &none, true).unwrap(),
            b"\x1b[<32;3;4M"
        );
        assert_eq!(
            report(WHEEL_UP, false, true, 0, 0, &none, true).unwrap(),
            b"\x1b[<64;1;1M"
        );
        assert_eq!(
            report(LEFT, false, true, 0, 0, &none, false).unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
        assert_eq!(
            report(LEFT, false, false, 0, 0, &none, false).unwrap(),
            vec![0x1b, b'[', b'M', 35, 33, 33]
        );
        assert_eq!(
            report(NO_BUTTON, true, true, 4, 1, &none, true).unwrap(),
            b"\x1b[<35;5;2M"
        );
        assert!(report(LEFT, false, true, 300, 0, &none, false).is_none());
    }
}
