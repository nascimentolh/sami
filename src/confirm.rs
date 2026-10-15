use gpui_kit::{prelude::FluentBuilder, *};

use crate::{form::button, theme::Theme};

pub enum Answer {
    Ok,
    Cancel,
}

/// O `confirmDlg` do protótipo: título, texto, Cancelar e a ação (vermelha quando é perigosa).
pub struct Confirm {
    title: String,
    text: String,
    ok: String,
    danger: bool,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Answer> for Confirm {}

impl Confirm {
    pub fn new(
        theme: Theme,
        title: String,
        text: String,
        ok: &str,
        danger: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            title,
            text,
            ok: ok.into(),
            danger,
            focus,
            theme,
        }
    }
}

impl Render for Confirm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let danger = self.danger;
        let red: Hsla = rgb(0xff5555).into();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                match e.keystroke.key.as_str() {
                    "escape" => cx.emit(Answer::Cancel),
                    // Enter confirma só quando não é perigoso (o foco do protótipo fica no Cancelar)
                    "enter" if !danger => cx.emit(Answer::Ok),
                    _ => {}
                }
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
            .text_size(px(13.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(div().text_color(t.muted).child(self.text.clone()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        button(t, "c-no", "Cancelar", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Answer::Cancel))),
                    )
                    .child(
                        button(t, "c-ok", &self.ok, !danger)
                            .when(danger, |d| d.text_color(red))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Answer::Ok))),
                    ),
            )
    }
}

/// `.toast` do protótipo: some sozinho.
pub fn toast(t: &Theme, msg: &str) -> Div {
    div()
        .bg(t.fg)
        .text_color(t.win)
        .px(px(12.))
        .py(px(8.))
        .rounded(px(6.))
        .shadow_lg()
        .text_size(px(12.5))
        .child(msg.to_string())
}
