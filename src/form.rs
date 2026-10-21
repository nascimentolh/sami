use std::ops::Range;

use gpui_kit::{prelude::FluentBuilder, *};

use crate::theme::Theme;

pub enum FieldEvent {
    Changed,
    /// Enter (linha única) ou Ctrl+Enter (as duas)
    Submit,
    Cancel,
}

/// Campo do `.dlg` do protótipo. O texto chega pelo input handler (acentos e teclas mortas funcionam).
// ponytail: cursor sempre no fim (digitar e apagar); seleção e setas quando algum formulário pedir
pub struct TextField {
    pub value: String,
    placeholder: &'static str,
    multiline: bool,
    mono: bool,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<FieldEvent> for TextField {}

impl TextField {
    pub fn new(
        theme: Theme,
        value: &str,
        placeholder: &'static str,
        multiline: bool,
        mono: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            value: value.into(),
            placeholder,
            multiline,
            mono,
            focus: cx.focus_handle(),
            theme,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn set_placeholder(&mut self, placeholder: &'static str, cx: &mut Context<Self>) {
        self.placeholder = placeholder;
        cx.notify();
    }

    pub fn set(&mut self, value: String, cx: &mut Context<Self>) {
        self.value = value;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "enter" if k.modifiers.control || !self.multiline => cx.emit(FieldEvent::Submit),
            "enter" => {
                self.value.push('\n');
                cx.emit(FieldEvent::Changed);
            }
            "escape" => cx.emit(FieldEvent::Cancel),
            "backspace" => {
                self.value.pop();
                cx.emit(FieldEvent::Changed);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.value.push_str(text);
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let focused = self.focus.is_focused(window);
        let entity = cx.entity();
        let focus = self.focus.clone();
        let fg = t.fg;
        let caret = move || div().w(px(1.)).h(px(15.)).bg(fg).flex_none();
        let mut body = div().flex().flex_col().min_w_0();
        if self.value.is_empty() {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .when(focused, |d| d.child(caret()))
                    .child(div().text_color(t.muted).child(self.placeholder)),
            );
        } else {
            let lines: Vec<&str> = self.value.split('\n').collect();
            let last = lines.len() - 1;
            let wrap = self.multiline;
            for (i, line) in lines.into_iter().enumerate() {
                // o GPUI só quebra texto em bloco; o cursor vai como caractere no fim da linha
                body = body.child(if wrap {
                    div()
                        .min_h(px(18.))
                        .whitespace_normal()
                        .child(if focused && i == last {
                            format!("{line}▏")
                        } else {
                            line.to_string()
                        })
                } else {
                    div()
                        .flex()
                        .items_center()
                        .min_h(px(18.))
                        .child(line.to_string())
                        .when(focused && i == last, |d| d.child(caret()))
                });
            }
        }
        div()
            .id("field")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                }),
            )
            .relative()
            .w_full()
            .px(px(8.))
            .py(px(6.))
            .bg(t.win)
            .border_1()
            .border_color(if focused { t.accent } else { t.line })
            .rounded(px(5.))
            .text_size(px(14.))
            .font_family(if self.mono {
                t.mono.clone()
            } else {
                t.sans.into()
            })
            .when(self.multiline, |d| d.min_h(px(64.)))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(body)
    }
}

pub fn checkbox(t: &Theme, on: bool) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded(px(3.))
        .border_1()
        .border_color(if on { t.accent } else { t.line })
        .when(on, |d| d.bg(t.accent))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .text_color(rgb(0x282a36))
        .when(on, |d| d.child("✓"))
}

pub fn button(t: &Theme, id: impl Into<ElementId>, label: &str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .gap(px(6.))
        .items_center()
        .px(px(10.))
        .py(px(3.))
        .rounded(px(5.))
        .border_1()
        .cursor_pointer()
        .flex_none()
        .whitespace_nowrap()
        .when(primary, |d| {
            d.bg(t.accent)
                .border_color(t.accent)
                .text_color(rgb(0x282a36))
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!primary, |d| {
            d.bg(t.raise)
                .border_color(t.line)
                .hover(|d| d.border_color(t.muted))
        })
        .child(label.to_string())
}
