use std::ops::Range;

use gpui_kit::{
    component::input::{Enter, Escape, Input, InputEvent, InputState, Textarea, TextareaState},
    prelude::FluentBuilder,
    *,
};

use crate::theme::Theme;

pub enum FieldEvent {
    Changed,
    /// Enter (linha única) ou Ctrl+Enter (as duas)
    Submit,
    Cancel,
}

enum Inner {
    Line(Entity<InputState>),
    Area(Entity<TextareaState>),
}

/// Campo do `.dlg` do protótipo sobre o input do gpui-component (colar, selecionar, setas, desfazer).
pub struct TextField {
    pub value: String,
    placeholder: &'static str,
    multiline: bool,
    mono: bool,
    /// o foco de fora: quem chama foca este antes de o input existir, e o render passa para dentro
    focus: FocusHandle,
    theme: Theme,
    // o input do gpui-component pede a Window, que boa parte dos construtores não tem: nasce no primeiro render
    inner: Option<Inner>,
    pending: Option<String>,
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
            inner: None,
            pending: None,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn set_placeholder(&mut self, placeholder: &'static str, cx: &mut Context<Self>) {
        self.placeholder = placeholder;
        self.pending.get_or_insert_with(|| self.value.clone());
        cx.notify();
    }

    pub fn set(&mut self, value: String, cx: &mut Context<Self>) {
        self.value = value.clone();
        self.pending = Some(value);
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    /// O cursor, em bytes de `value`.
    pub fn cursor(&self, cx: &App) -> usize {
        match &self.inner {
            Some(Inner::Line(s)) => s.read(cx).cursor(),
            Some(Inner::Area(s)) => s.read(cx).cursor(),
            None => self.value.len(),
        }
    }

    /// Troca `range` de `value` por `text` e põe o cursor logo depois.
    pub fn splice(
        &mut self,
        range: Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.value.replace_range(range.clone(), text);
        let at = range.start + text.len();
        let v = self.value.clone();
        match &self.inner {
            Some(Inner::Line(s)) => s.update(cx, |s, cx| {
                s.set_value(v, window, cx);
                s.set_selected_range(at..at, cx);
            }),
            Some(Inner::Area(s)) => s.update(cx, |s, cx| {
                s.set_value(v, window, cx);
                s.set_selected_range(at..at, cx);
            }),
            None => self.pending = Some(v),
        }
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn on_input(&mut self, e: &InputEvent, text: String, cx: &mut Context<Self>) {
        if matches!(e, InputEvent::Change) {
            self.value = text;
            cx.emit(FieldEvent::Changed);
        }
    }

    fn inner_focus(&self, cx: &App) -> Option<FocusHandle> {
        match self.inner.as_ref()? {
            Inner::Line(s) => Some(s.read(cx).focus_handle(cx)),
            Inner::Area(s) => Some(s.read(cx).focus_handle(cx)),
        }
    }

    fn ensure_inner(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.inner.is_none() {
            let (v, ph) = (self.value.clone(), self.placeholder);
            self.inner = Some(if self.multiline {
                let s = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(3, 10)
                        .placeholder(ph)
                        .default_value(v)
                });
                cx.subscribe(&s, |this, s, e: &InputEvent, cx| {
                    let text = s.read(cx).value().to_string();
                    this.on_input(e, text, cx)
                })
                .detach();
                Inner::Area(s)
            } else {
                let s = cx.new(|cx| InputState::new(window, cx).placeholder(ph).default_value(v));
                cx.subscribe(&s, |this, s, e: &InputEvent, cx| {
                    let text = s.read(cx).value().to_string();
                    this.on_input(e, text, cx)
                })
                .detach();
                Inner::Line(s)
            });
            self.pending = None;
        }
        if let Some(v) = self.pending.take() {
            let ph = self.placeholder;
            match &self.inner {
                Some(Inner::Line(s)) => s.update(cx, |s, cx| {
                    s.set_placeholder(ph, window, cx);
                    s.set_value(v, window, cx);
                }),
                Some(Inner::Area(s)) => s.update(cx, |s, cx| {
                    s.set_placeholder(ph, window, cx);
                    s.set_value(v, window, cx);
                }),
                None => {}
            }
        }
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_inner(window, cx);
        if self.focus.is_focused(window) {
            cx.defer_in(window, |this, window, cx| {
                let end = this.value.len();
                match &this.inner {
                    Some(Inner::Line(s)) => {
                        s.update(cx, |s, cx| s.set_selected_range(end..end, cx))
                    }
                    Some(Inner::Area(s)) => {
                        s.update(cx, |s, cx| s.set_selected_range(end..end, cx))
                    }
                    None => {}
                }
                if let Some(f) = this.inner_focus(cx) {
                    window.focus(&f, cx);
                }
            });
        }
        let t = &self.theme;
        let focused = self.focus.contains_focused(window, cx);
        let font: SharedString = if self.mono {
            t.mono.clone().into()
        } else {
            t.sans.into()
        };
        let input = match self.inner.as_ref() {
            Some(Inner::Line(s)) => Input::new(s)
                .appearance(false)
                .font_family(font)
                .text_size(px(14.))
                .into_any_element(),
            Some(Inner::Area(s)) => Textarea::new(s)
                .appearance(false)
                .font_family(font)
                .text_size(px(14.))
                .into_any_element(),
            None => div().into_any_element(),
        };
        div()
            .id("field")
            .track_focus(&self.focus)
            // as teclas do input viram ações dele antes de qualquer key_down: Enter e Esc são pegos na captura
            .capture_action(cx.listener(|this, e: &Enter, _, cx| {
                if e.secondary || !this.multiline {
                    cx.emit(FieldEvent::Submit);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.emit(FieldEvent::Cancel);
                cx.stop_propagation();
            }))
            .w_full()
            .min_w_0()
            .px(px(4.))
            .py(px(2.))
            .bg(t.win)
            .border_1()
            .border_color(if focused { t.accent } else { t.line })
            .rounded(px(5.))
            .text_color(t.fg)
            .child(input)
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
