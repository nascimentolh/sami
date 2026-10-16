use std::path::PathBuf;

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    feature::{self, Orphan},
    form::{button, checkbox},
    sidebar::tilde,
    theme::Theme,
};

pub enum Event {
    Done(Option<String>),
}

/// "Limpeza": o que está no disco e nenhuma feature reconhece, para apagar com confirmação.
pub struct Cleanup {
    items: Option<Vec<(Orphan, bool)>>,
    busy: bool,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Event> for Cleanup {}

impl Cleanup {
    pub fn new(
        repos: Vec<PathBuf>,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { feature::orphans(&repos) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.items = Some(
                    found
                        .into_iter()
                        .map(|o| {
                            let s = o.safe();
                            (o, s)
                        })
                        .collect(),
                );
                cx.notify();
            });
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            items: None,
            busy: false,
            focus,
            theme,
        }
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let Some(items) = &self.items else { return };
        let chosen: Vec<Orphan> = items
            .iter()
            .filter(|(_, on)| *on)
            .map(|(o, _)| o.clone())
            .collect();
        if chosen.is_empty() {
            return cx.emit(Event::Done(None));
        }
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let n = chosen.len();
            let errors: Vec<String> = cx
                .background_executor()
                .spawn(async move { chosen.iter().filter_map(|o| o.remove().err()).collect() })
                .await;
            let msg = if errors.is_empty() {
                format!(
                    "Limpeza: {n} ite{} apagado{}",
                    if n > 1 { "ns" } else { "m" },
                    if n > 1 { "s" } else { "" }
                )
            } else {
                format!("Limpeza: {} falharam ({})", errors.len(), errors[0])
            };
            let _ = this.update(cx, |_, cx| cx.emit(Event::Done(Some(msg))));
        })
        .detach();
    }
}

impl Render for Cleanup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .max_h(px(340.))
            .overflow_hidden();
        let count = self
            .items
            .as_ref()
            .map_or(0, |i| i.iter().filter(|(_, on)| *on).count());
        match &self.items {
            None => list = list.child(div().text_color(t.muted).child("Procurando…")),
            Some(items) if items.is_empty() => {
                list = list.child(
                    div()
                        .text_color(t.muted)
                        .child("Nada sobrando: tudo que existe pertence a uma feature."),
                )
            }
            Some(items) => {
                for (i, (o, on)) in items.iter().enumerate() {
                    let (name, why) = o.label();
                    list = list.child(
                        div()
                            .id(i)
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(it) = this.items.as_mut().and_then(|v| v.get_mut(i)) {
                                    it.1 = !it.1;
                                }
                                cx.notify();
                            }))
                            .child(checkbox(&t, *on))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family(t.mono.clone())
                                            .text_size(px(13.))
                                            .child(tilde(&name)),
                                    )
                                    .child(div().text_size(px(13.)).text_color(t.muted).child(why)),
                            ),
                    );
                }
            }
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| match e.keystroke.key.as_str() {
                "escape" => cx.emit(Event::Done(None)),
                "enter" => this.apply(cx),
                _ => {}
            }))
            .w(px(480.))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Limpeza"))
            .child(div().text_color(t.muted).child(
                "O que está no disco e nenhuma feature reconhece. Branch local e worktree com mudanças vêm desmarcadas.",
            ))
            .child(list)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(button(&t, "cl-no", "Cancelar", false).on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Done(None)))))
                    .when(count > 0, |d| {
                        d.child(
                            button(&t, "cl-ok", &if self.busy { "Apagando…".to_string() } else { format!("Apagar {count}") }, false)
                                .text_color(rgb(0xff5555))
                                .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                        )
                    }),
            )
    }
}
