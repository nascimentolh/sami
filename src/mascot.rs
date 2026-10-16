use std::{path::PathBuf, time::Duration};

use gpui_kit::*;
use serde_json::Value;

use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Ask,
    Done,
}

/// Um pedido de atenção na fila do mascote (o `S.mq` do protótipo).
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub pane: String,
    pub kind: Kind,
    pub branch: String,
    pub text: String,
}

/// As poses do protótipo, as mesmas de `claude-mascot/overlay/poses.py`.
const FRONT: &str = "...............
...............
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..OKKKOOOKKKO..
.OOKKKOOOKKKOO.
.OOOOOOOOOOOOO.
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..O.O.....O.O..
..O.O.....O.O..";
const BLINK: &str = "...............
...............
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..OOOOOOOOOOO..
.OOKKKOOOKKKOO.
.OOOOOOOOOOOOO.
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..O.O.....O.O..
..O.O.....O.O..";
const HAPPY: &str = "...............
...............
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..OOKOOOOOKOO..
.OOKKKOOOKKKOO.
.OOOOOOOOOOOOO.
..OOOOOOOOOOO..
..OOOOOOOOOOO..
..O.O.....O.O..
..O.O.....O.O..";

/// O `sprite()` do protótipo: um quadrado de `c` px por caractere (`INK`: O laranja, K preto).
fn sprite(pose: &str, c: f32) -> Div {
    let mut grid = div().flex().flex_col();
    for row in pose.lines() {
        let mut r = div().flex();
        for ch in row.chars() {
            let cell = div().size(px(c));
            r = r.child(match ch {
                'O' => cell.bg(rgb(0xD97757)),
                'K' => cell.bg(rgb(0x191919)),
                _ => cell,
            });
        }
        grid = grid.child(r);
    }
    grid
}

/// O `.mascot` do protótipo: card com a feature e o bicho pulando no canto.
pub fn view(
    t: &Theme,
    n: &Note,
    queued: usize,
    frame: u64,
    seq: usize,
    leaving: bool,
) -> AnimationElement<Div> {
    let ask = n.kind == Kind::Ask;
    let pose = if frame % 6 == 5 {
        BLINK
    } else if ask {
        FRONT
    } else {
        HAPPY
    };
    let go = format!(
        "clique para ir · Esc dispensa{}",
        if queued > 0 {
            format!(" · +{queued} na fila")
        } else {
            String::new()
        }
    );
    let body = div()
        .relative()
        .child(sprite(pose, 5.))
        .child(
            div()
                .absolute()
                .right(px(-6.))
                .top(px(-22.))
                .size(px(22.))
                .rounded_full()
                .bg(if ask { rgb(0x6B4FD8) } else { rgb(0x2BA84A) })
                .text_color(rgb(0xffffff))
                .font_family(t.mono.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(px(17.))
                .flex()
                .items_center()
                .justify_center()
                .child(if ask { "?" } else { "✓" }),
        )
        .with_animation(
            ("hop", seq),
            Animation::new(Duration::from_millis(1000))
                .repeat()
                .with_easing(bounce(ease_in_out)),
            |d, v| d.top(px(-8. * v)),
        );
    let el = div()
        .relative()
        .flex()
        .items_end()
        .gap(px(10.))
        .cursor_pointer()
        .child(
            div()
                .mb(px(34.))
                .max_w(px(260.))
                .px(px(10.))
                .py(px(8.))
                .bg(t.raise)
                .border_1()
                .border_color(t.line)
                .rounded(px(8.))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .child(n.branch.clone()),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child(n.text.clone()),
                )
                .child(div().text_size(px(12.5)).text_color(t.accent).child(go)),
        )
        .child(
            div().child(body).child(
                div()
                    .w(px(56.))
                    .h(px(6.))
                    .mt(px(2.))
                    .mx_auto()
                    .rounded_full()
                    .bg(rgba(0x00000059)),
            ),
        );
    // walkin .9s steps(6) e walkout .7s steps(5), como no CSS do protótipo
    let (ms, steps, out) = if leaving {
        (700, 5., true)
    } else {
        (900, 6., false)
    };
    el.with_animation(
        (if out { "walkout" } else { "walkin" }, seq),
        Animation::new(Duration::from_millis(ms)),
        move |d, v| {
            let v = (v * steps).floor() / steps;
            if out {
                d.left(px(180. * v)).opacity(1. - v)
            } else {
                d.left(px(160. * (1. - v))).opacity(v)
            }
        },
    )
}

/// O som do `claude-mascot/overlay/config.py`, com o mesmo config (`~/.config/claude-mascot/config.json`).
/// Com o plugin claude-mascot ligado, ele já toca o som do mesmo pedido: aqui fica mudo para não tocar duas vezes.
pub fn play(kind: Kind) {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let read = |p: PathBuf| -> Value {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    };
    let settings = read(home.join(".claude/settings.json"));
    if settings["enabledPlugins"]["claude-mascot@claude-reminder"] == true {
        return;
    }
    let cfg = read(
        std::env::var_os("XDG_CONFIG_HOME")
            .map_or(home.join(".config"), PathBuf::from)
            .join("claude-mascot/config.json"),
    );
    let state = if kind == Kind::Ask { "ask" } else { "done" };
    if cfg["mute"] == true || cfg["enabled"] == false || cfg["states"][state] == false {
        return;
    }
    let path = cfg["sounds"][state]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| {
            format!(
                "/usr/share/sounds/freedesktop/stereo/{}.oga",
                if kind == Kind::Ask {
                    "message-new-instant"
                } else {
                    "complete"
                }
            )
        });
    let volume = cfg["volume"].as_f64().unwrap_or(1.0).to_string();
    let _ = std::process::Command::new("pw-play")
        .args(["--volume", &volume, &path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
