use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::theme::Theme;

/// Onde cada alvo do tutorial foi desenhado no último frame.
pub type Anchors = Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>;

/// Marca o elemento pai como alvo `id` (vai como filho dele, ocupando o mesmo espaço).
pub fn anchor(anchors: &Anchors, id: &'static str) -> impl IntoElement {
    let anchors = anchors.clone();
    canvas(
        move |bounds, _, _| {
            anchors.borrow_mut().insert(id, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub enum Part {
    T(&'static str),
    B(&'static str),
    K(&'static str),
    M(&'static str),
}
use Part::*;

pub struct Step {
    pub target: &'static str,
    pub text: &'static [Part],
    /// o passo termina com o clique no próprio alvo (sem "Próximo")
    pub wait: bool,
}

/// Os passos do `TOURS.feat` do protótipo.
pub const FEAT: &[Step] = &[
    Step {
        target: "fila",
        text: &[
            T("A"),
            B("Fila"),
            T(
                "junta tudo que espera por você: permissões, CI vermelho, comentários, revisões, a daily.",
            ),
            K("Ctrl+Shift+Espaço"),
            T("leva ao próximo."),
        ],
        wait: false,
    },
    Step {
        target: "features",
        text: &[
            T("Cada linha é uma"),
            B("feature:"),
            T(
                "uma worktree com o próprio Claude, porta própria e custo. O símbolo mostra o estado.",
            ),
        ],
        wait: false,
    },
    Step {
        target: "term",
        text: &[
            T("Terminal de verdade (herdr). Aba 1 é o Claude, aba 2 é zsh."),
            B("Ctrl+tecla"),
            T("vai para o terminal;"),
            B("Ctrl+Shift+tecla"),
            T("é do Maestro."),
        ],
        wait: false,
    },
    Step {
        target: "newf",
        text: &[T(
            "Crie uma feature: branch, issue do Jira, até 3 tentativas em paralelo. Clique para ver.",
        )],
        wait: true,
    },
];

/// Os passos do `TOURS.review` do protótipo.
pub const REVIEW: &[Step] = &[
    Step {
        target: "rtool",
        text: &[
            T("Escolha o"),
            B("turno"),
            T(
                "do Claude para ver só o que mudou naquele passo, e volte para antes dele se precisar.",
            ),
        ],
        wait: false,
    },
    Step {
        target: "hunkacts",
        text: &[
            T("Cada bloco tem"),
            B("Aceitar"),
            T("e"),
            B("Desfazer."),
            T("As linhas são editáveis aqui mesmo: é um multibuffer, como no Zed."),
        ],
        wait: false,
    },
    Step {
        target: "send",
        text: &[
            T("Comentários (tecla"),
            K("C"),
            T(") e suas edições vão juntos para o Claude desta feature."),
        ],
        wait: false,
    },
];

/// O `TOURS.mrtab` do protótipo.
pub const MRTAB: &[Step] = &[Step {
    target: "mrcard",
    text: &[
        T(
            "O MR da feature: pipeline, Sonar e comentários. Cada problema tem um botão que chama a skill certa (",
        ),
        M("/pipe"),
        T(","),
        M("/comentario"),
        T(")."),
    ],
    wait: false,
}];

fn state_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/maestro/state.toml")
}

pub fn seen(id: &str) -> bool {
    std::fs::read_to_string(state_path())
        .is_ok_and(|s| s.lines().any(|l| l.trim() == format!("tour_{id} = true")))
}

pub fn mark(id: &str) {
    let path = state_path();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text
        .lines()
        .any(|l| l.trim() == format!("tour_{id} = true"))
    {
        text.push_str(&format!("tour_{id} = true\n"));
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, text);
    }
}

/// Destaque + balão do passo `i`; `None` se o alvo ainda não foi desenhado.
pub fn overlay(
    steps: &'static [Step],
    i: usize,
    anchors: &Anchors,
    viewport: Size<Pixels>,
    t: &Theme,
    on_next: impl Fn(&mut Window, &mut App) + 'static,
    on_skip: impl Fn(&mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    let step = steps.get(i)?;
    let r = *anchors.borrow().get(step.target)?;
    let (x, y) = (r.origin.x - px(4.), r.origin.y - px(4.));
    let (w, h) = (
        r.size.width + px(8.),
        (r.size.height + px(8.)).min(viewport.height - y - px(8.)),
    );
    let shade = Hsla::from(rgba(0x08080e9e));
    let below = y + h + px(170.) < viewport.height;
    let pop_x = px(12.).max(x.min(viewport.width - px(312.)));
    let pop_y = if below {
        y + h + px(10.)
    } else {
        px(12.).max(y - px(170.))
    };
    let last = i + 1 == steps.len();

    let mut words = div()
        .flex()
        .flex_wrap()
        .gap_x(px(4.))
        .gap_y(px(2.))
        .items_center();
    for part in step.text {
        words = match part {
            T(s) => s.split(' ').fold(words, |d, w| d.child(w.to_string())),
            B(s) => s.split(' ').fold(words, |d, w| {
                d.child(div().font_weight(FontWeight::BOLD).child(w.to_string()))
            }),
            K(s) => words.child(
                div()
                    .font_family(t.mono.clone())
                    .text_size(px(11.))
                    .px(px(4.))
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(3.))
                    .text_color(t.muted)
                    .bg(t.panel)
                    .child(s.to_string()),
            ),
            M(s) => words.child(div().font_family(t.mono.clone()).child(s.to_string())),
        };
    }
    let btn = |id: &'static str, label: &'static str, pri: bool| {
        div()
            .id(id)
            .px(px(10.))
            .py(px(3.))
            .rounded(px(5.))
            .border_1()
            .cursor_pointer()
            .whitespace_nowrap()
            .when(pri, |d| {
                d.bg(t.accent)
                    .border_color(t.accent)
                    .text_color(rgb(0x282a36))
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!pri, |d| {
                d.bg(t.raise)
                    .border_color(t.line)
                    .hover(|d| d.border_color(t.muted))
            })
            .child(label)
    };
    let rect = |l: Pixels, tp: Pixels, w: Pixels, h: Pixels| {
        div().absolute().left(l).top(tp).w(w).h(h).bg(shade)
    };
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // o escuro em volta do alvo (o box-shadow de 9999px do protótipo)
            .child(rect(px(0.), px(0.), viewport.width, y.max(px(0.))))
            .child(rect(px(0.), y + h, viewport.width, viewport.height - y - h))
            .child(rect(px(0.), y, x.max(px(0.)), h))
            .child(rect(x + w, y, viewport.width - x - w, h))
            .child(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(w)
                    .h(h)
                    .rounded(px(6.))
                    .border_2()
                    .border_color(t.accent),
            )
            .child(
                div()
                    .id("tour-pop")
                    .absolute()
                    .left(pop_x)
                    .top(pop_y)
                    .w(px(300.))
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .bg(t.raise)
                    .border_1()
                    .border_color(t.accent)
                    .rounded(px(8.))
                    .shadow_lg()
                    .child(words)
                    .when(step.wait, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.accent)
                                .child("↳ clique no destaque para continuar"),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_size(px(11.))
                                    .text_color(t.muted)
                                    .child(format!("{}/{}", i + 1, steps.len())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .child(
                                        btn("tour-skip", "Pular tutorial", false)
                                            .on_click(move |_, w, cx| on_skip(w, cx)),
                                    )
                                    .when(!step.wait, |d| {
                                        d.child(
                                            btn(
                                                "tour-next",
                                                if last { "Entendi" } else { "Próximo" },
                                                true,
                                            )
                                            .on_click(move |_, w, cx| on_next(w, cx)),
                                        )
                                    }),
                            ),
                    ),
            )
            .into_any_element(),
    )
}
