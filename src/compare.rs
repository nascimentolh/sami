use gpui_kit::{prelude::FluentBuilder, *};

use crate::{feature::Inventory, form::button, sidebar::brl, theme::Theme};

/// Uma tentativa como o `compareDlg` do protótipo mostra.
pub struct Attempt {
    pub inv: Inventory,
    pub state: (String, Hsla),
    pub done: bool,
    pub files: usize,
    pub added: u32,
    pub removed: u32,
    pub cost: Option<f64>,
}

pub enum Event {
    Close,
    Keep(usize),
    /// "Ver diff": a feature da tentativa com o painel Revisar aberto
    Diff(usize),
}

pub struct Compare {
    group: String,
    attempts: Vec<Attempt>,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Event> for Compare {}

impl Compare {
    pub fn new(
        group: String,
        attempts: Vec<Attempt>,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            group,
            attempts,
            focus,
            theme,
        }
    }
}

impl Render for Compare {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let mixed = self
            .attempts
            .iter()
            .any(|a| a.inv.model != self.attempts[0].inv.model);
        let mut cols = div().flex().gap(px(10.));
        for (i, a) in self.attempts.iter().enumerate() {
            let row = |k: &str, v: Div| {
                div()
                    .flex()
                    .gap(px(10.))
                    .text_size(px(12.))
                    .child(div().w(px(56.)).text_color(t.muted).child(k.to_string()))
                    .child(v)
            };
            let model = a
                .inv
                .model
                .clone()
                .unwrap_or_else(|| "padrão do claude".into());
            cols =
                cols.child(
                    div()
                        .flex_1()
                        .min_w(px(200.))
                        .p(px(10.))
                        .border_1()
                        .border_color(t.line)
                        .rounded(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .child(a.inv.branch.clone()),
                        )
                        .child(row("modelo", div().child(model)))
                        .child(row(
                            "estado",
                            div().text_color(a.state.1).child(a.state.0.clone()),
                        ))
                        .child(row(
                            "arquivos",
                            div().child(format!("{} · +{} −{}", a.files, a.added, a.removed)),
                        ))
                        // ponytail: testes por tentativa chegam com os "testes afetados" da Etapa 4
                        .child(row("testes", div().child("—")))
                        .child(row(
                            "custo",
                            div().child(a.cost.map_or("—".into(), |c| format!("US$ {}", brl(c)))),
                        ))
                        .child(
                            div()
                                .mt(px(6.))
                                .flex()
                                .gap(px(6.))
                                .child(button(&t, ("cv", i), "Ver diff", false).on_click(
                                    cx.listener(move |_, _, _, cx| cx.emit(Event::Diff(i))),
                                ))
                                .child(
                                    button(&t, ("keep", i), "Ficar com esta", true)
                                        .when(!a.done, |d| d.opacity(0.45))
                                        .when(a.done, |d| {
                                            d.on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(Event::Keep(i))
                                            }))
                                        }),
                                ),
                        ),
                );
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(Event::Close);
                }
            }))
            .w(px(780.))
            .max_w_full()
            .px(px(20.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(13.))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(format!("Comparar tentativas · {}", self.group)))
            .child(div().text_color(t.muted).child(format!(
                "Mesmo prompt, worktrees separadas{}. Escolha uma; as outras worktrees são removidas.",
                if mixed { ", modelos diferentes" } else { "" }
            )))
            .child(cols)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .child(button(&t, "cmp-close", "Fechar", false).on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Close)))),
            )
    }
}
