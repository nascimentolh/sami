use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    herdr::{self, Status},
    keys,
    sidebar::Sidebar,
    terminal::TerminalView,
    theme::Theme,
};

/// Janela do protótipo: título (36) · lateral | terminal · dicas (22) · status (24).
pub struct Shell {
    pub sidebar: Option<Entity<Sidebar>>,
    pub terminal: Entity<TerminalView>,
    pub theme: Theme,
    pub font: String,
}

/// Dicas do modo Features do protótipo; `false` = ainda não existe e aparece apagada.
const HINTS: &[(&str, &str, bool)] = &[
    ("Ctrl+Shift+Espaço", "próximo da fila", true),
    ("Ctrl+Shift+1…9", "feature", true),
    ("Ctrl+Shift+E", "revisar mudanças", false),
    ("Ctrl+Shift+\\", "dividir", true),
    ("Ctrl+Shift+Enter", "empilhar", true),
    ("Ctrl+Shift+←→", "trocar pane", true),
    ("Ctrl+A", "herdr", true),
    ("Ctrl+Shift+L", "seleção → Claude", false),
    ("Alt+N", "nova feature", false),
    ("?", "atalhos", false),
];

impl Shell {
    pub fn new(
        sidebar: Option<Entity<Sidebar>>,
        terminal: Entity<TerminalView>,
        theme: Theme,
        font: String,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(s) = &sidebar {
            cx.observe(s, |_, _, cx| cx.notify()).detach();
        }
        Self {
            sidebar,
            terminal,
            theme,
            font,
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let Some(sidebar) = &self.sidebar else { return };
        let s = sidebar.read(cx);
        let target = if let Some(n) = keys::feature_digit(k) {
            s.order().into_iter().nth(n - 1)
        } else if m.control && m.shift && !m.alt && k.key == "space" {
            s.next_attention()
        } else {
            return;
        };
        if let Some(pane) = target {
            s.focus(pane);
        }
        cx.stop_propagation();
    }

    fn kbd(&self, k: &str) -> Div {
        let t = &self.theme;
        div()
            .font_family(t.mono.clone())
            .text_size(px(11.))
            .px(px(4.))
            .border_1()
            .border_color(t.line)
            .rounded(px(3.))
            .text_color(t.muted)
            .bg(t.panel)
            .whitespace_nowrap()
            .child(k.to_string())
    }

    fn title(&self, cx: &App) -> Div {
        let t = &self.theme;
        let (blocked, attention) = self.sidebar.as_ref().map_or((0, 0), |s| {
            let a = &s.read(cx).agents;
            let n = |st: &[Status]| a.iter().filter(|x| st.contains(&x.agent_status)).count();
            (n(&[Status::Blocked]), n(&[Status::Blocked, Status::Done]))
        });
        let mode = |label: &str, on: bool, count: Option<String>| {
            div()
                .flex()
                .gap(px(6.))
                .items_center()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .text_color(if on { t.fg } else { t.muted })
                .when(on, |d| d.bg(t.raise))
                // ponytail: MRs e Vault são as Etapas 5 e 6; até lá ficam visíveis e apagados
                .when(!on, |d| d.opacity(0.45))
                .child(label.to_string())
                .when_some(count, |d, c| {
                    d.child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(11.))
                            .text_color(t.orange)
                            .child(c),
                    )
                })
        };
        div()
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.line)
            .child(div().font_weight(FontWeight::SEMIBOLD).child("maestro"))
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .ml(px(8.))
                    .child(mode(
                        "Features",
                        true,
                        (blocked > 0).then(|| format!("{blocked} !")),
                    ))
                    .child(mode("MRs", false, None))
                    .child(mode("Vault", false, None)),
            )
            .child(
                div()
                    .id("fila")
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(t.line)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.raise))
                    .on_click({
                        let sidebar = self.sidebar.clone();
                        move |_, _, cx| {
                            if let Some(s) = &sidebar {
                                let s = s.read(cx);
                                if let Some(p) = s.next_attention() {
                                    s.focus(p);
                                }
                            }
                        }
                    })
                    .child("Fila")
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.orange)
                            .child(attention.to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .text_color(t.muted)
                    .opacity(0.45)
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(self.kbd("Ctrl+Shift+O"))
                            .child("arquivo"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(self.kbd("Ctrl+Shift+P"))
                            .child("comandos"),
                    )
                    .child(self.kbd("?")),
            )
    }

    fn hints(&self) -> Div {
        let t = &self.theme;
        let mut bar = div()
            .h(px(22.))
            .flex_none()
            .flex()
            .gap(px(14.))
            .items_center()
            .px(px(12.))
            .text_size(px(11.5))
            .text_color(t.muted)
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .overflow_hidden()
            .whitespace_nowrap();
        for (k, label, ready) in HINTS {
            bar = bar.child(
                div()
                    .flex()
                    .gap(px(4.))
                    .items_center()
                    .when(!ready, |d| d.opacity(0.45))
                    .child(self.kbd(k))
                    .child(label.to_string()),
            );
        }
        bar
    }

    fn status(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let cell = || {
            div()
                .h_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .border_r_1()
                .border_color(t.line)
        };
        let mut bar = div()
            .h(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .text_size(px(12.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .overflow_hidden()
            .whitespace_nowrap();
        match &self.sidebar {
            None => bar = bar.child(cell().child("herdr: desligado")),
            Some(s) => {
                let s = s.read(cx);
                let blocked = s
                    .agents
                    .iter()
                    .filter(|a| a.agent_status == Status::Blocked)
                    .count();
                let working = s
                    .agents
                    .iter()
                    .filter(|a| a.agent_status == Status::Working)
                    .count();
                bar = bar.child(cell().child(if s.offline {
                    "herdr: fora do ar"
                } else {
                    "herdr: conectado"
                }));
                if blocked > 0 {
                    let sidebar = self.sidebar.clone().unwrap();
                    bar = bar.child(
                        cell()
                            .id("att")
                            .text_color(t.orange)
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                let s = sidebar.read(cx);
                                if let Some(p) = s.next_attention() {
                                    s.focus(p);
                                }
                            })
                            .child(format!(
                                "! {blocked} precisa{} de você",
                                if blocked > 1 { "m" } else { "" }
                            )),
                    );
                }
                bar = bar.child(cell().child(format!("{working} trabalhando")));
                let focused = s.agents.iter().find(|a| a.focused);
                if let Some(b) = focused.and_then(|a| {
                    herdr::repo_root(std::path::Path::new(&a.cwd)).and_then(herdr::branch)
                }) {
                    bar = bar.child(cell().child(format!("⎇ {b}")));
                }
            }
        }
        bar.child(div().flex_1())
            .child(cell().border_r_0().child(format!("ghostty: {}", self.font)))
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(13.))
            .child(self.title(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(self.sidebar.clone())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.terminal.clone()),
                    ),
            )
            .child(self.hints())
            .child(self.status(cx))
    }
}
