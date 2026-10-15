/// Pontos de um caractere braille (U+2800..U+28FF) como (x, y, lado) em pixels dentro da célula.
/// Mesma geometria do sprite do Ghostty, que não usa o glifo da fonte.
pub fn dots(c: char, cell_w: u32, cell_h: u32) -> Option<Vec<(u32, u32, u32)>> {
    let bits = (c as u32).checked_sub(0x2800).filter(|b| *b <= 0xff)?;
    let mut w = (cell_w / 4).min(cell_h / 8);
    let (mut xs, mut ys) = (cell_w / 4, cell_h / 8);
    let (mut xm, mut ym) = (xs / 2, ys / 2);
    let mut x_left = cell_w.saturating_sub(2 * xm + xs + 2 * w);
    let mut y_left = cell_h.saturating_sub(2 * ym + 3 * ys + 4 * w);
    if x_left >= 2 && y_left >= 4 && w == 0 {
        w += 1;
        x_left -= 2;
        y_left -= 4;
    }
    if x_left >= 2 && xm == 0 {
        xm += 1;
        x_left -= 2;
    }
    if y_left >= 2 && ym == 0 {
        ym += 1;
        y_left -= 2;
    }
    if x_left >= 1 {
        xs += 1;
        x_left -= 1;
    }
    if y_left >= 3 {
        ys += 1;
        y_left -= 3;
    }
    if x_left >= 2 {
        xm += 1;
        x_left -= 2;
    }
    if y_left >= 2 {
        ym += 1;
        y_left -= 2;
    }
    if x_left >= 2 && y_left >= 4 {
        w += 1;
    }
    let x = [xm, xm + w + xs];
    let y = [ym, ym + w + ys, ym + 2 * (w + ys), ym + 3 * (w + ys)];
    // ordem dos bits do Unicode: 1-2-3 e 4-5-6 descem as colunas; 7 e 8 são a última linha
    const POS: [(usize, usize); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    Some(
        POS.iter()
            .enumerate()
            .filter(|(i, _)| bits & (1 << i) != 0)
            .map(|(_, &(cx, cy))| (x[cx], y[cy], w))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pontos_na_celula_do_ghostty() {
        assert_eq!(dots('a', 10, 22), None);
        assert_eq!(dots('\u{2800}', 10, 22), Some(vec![]));
        assert_eq!(dots('\u{2801}', 10, 22), Some(vec![(1, 2, 2)]));
        assert_eq!(dots('\u{2880}', 10, 22), Some(vec![(6, 17, 2)]));
        assert_eq!(dots('\u{28ff}', 10, 22).unwrap().len(), 8);
    }
}
