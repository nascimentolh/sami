use std::{ops::Range, path::PathBuf, rc::Rc, time::Duration};

use gpui_kit::{
    base::input::{FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope},
    component::input::{Editor, EditorState, InputEvent},
    prelude::FluentBuilder,
    *,
};

use crate::{
    diff::{self, Tok},
    form::button,
    theme::Theme,
};

pub enum Event {
    Changed,
    Toast(String),
    /// "Ver diff" do banner de conflito
    ShowDiff(String),
}

/// O editor do `openFile` do protótipo: o arquivo da worktree, salvo com Ctrl+S.
pub struct FileEditor {
    pub path: String,
    file: PathBuf,
    state: Entity<EditorState>,
    /// o que está no disco, pela última leitura ou gravação nossa
    saved: String,
    /// o disco mudou por fora enquanto havia edição sua
    conflict: Option<String>,
    theme: Theme,
    _poll: Task<()>,
}

impl EventEmitter<Event> for FileEditor {}

#[derive(Debug, PartialEq)]
enum Sync {
    Same,
    Reload,
    Conflict,
}

fn sync(saved: &str, buffer: &str, disk: &str) -> Sync {
    if disk == saved || disk == buffer {
        Sync::Same
    } else if buffer == saved {
        Sync::Reload
    } else {
        Sync::Conflict
    }
}

/// `@arquivo:a-b` da seleção, como o `sendSel` do protótipo.
pub fn line_ref(path: &str, text: &str, sel: Range<usize>) -> String {
    let line = |o: usize| text[..o.min(text.len())].matches('\n').count() + 1;
    let (a, b) = (line(sel.start), line(sel.end));
    if a == b {
        format!("@{path}:{a}")
    } else {
        format!("@{path}:{a}-{b}")
    }
}

pub fn style(t: &Theme, tok: Tok) -> HighlightStyle {
    HighlightStyle {
        color: Some(match tok {
            Tok::Comment => t.comment,
            Tok::Str => rgb(0xf1fa8c).into(),
            Tok::Keyword => rgb(0xff79c6).into(),
            Tok::Number => t.accent,
        }),
        font_style: (tok == Tok::Comment).then_some(FontStyle::Italic),
        ..Default::default()
    }
}

/// O `hl()` do protótipo dentro do editor do gpui-component.
// ponytail: realce por linha sem parser, relendo o arquivo do início a cada pintura (O(arquivo)); tree-sitter do gpui-component quando um arquivo grande pesar
struct Highlighter {
    text: String,
    theme: Theme,
}

impl InputHighlighter for Highlighter {
    fn language(&self) -> SharedString {
        "maestro".into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.text = text.to_string();
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut out = Vec::new();
        let mut at = range.start;
        let mut start = 0;
        for line in self.text.split_inclusive('\n') {
            let end = start + line.len();
            if end > range.start && start < range.end {
                for (r, tok) in diff::highlight(line.trim_end_matches('\n')) {
                    let r = (r.start + start).max(range.start)..(r.end + start).min(range.end);
                    if r.start >= r.end {
                        continue;
                    }
                    if r.start > at {
                        out.push((at..r.start, HighlightStyle::default()));
                    }
                    at = r.end;
                    out.push((r, style(&self.theme, tok)));
                }
            }
            if end >= range.end {
                break;
            }
            start = end;
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

impl FileEditor {
    pub fn new(
        root: PathBuf,
        path: String,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file = root.join(&path);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let t = theme.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .line_number(true)
                .indent_guides(false)
                .default_value(text.clone());
            s.set_highlighter_factory(
                Rc::new(move |_| {
                    Some(Box::new(Highlighter {
                        text: String::new(),
                        theme: t.clone(),
                    }) as Box<dyn InputHighlighter>)
                }),
                cx,
            );
            s
        });
        cx.subscribe(&state, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.emit(Event::Changed);
            }
        })
        .detach();
        // ponytail: lê o arquivo a cada 1 s; `notify` quando houver muitos editores abertos
        let poll = cx.spawn_in(window, {
            let file = file.clone();
            async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let f = file.clone();
                let disk = cx
                    .background_executor()
                    .spawn(async move { std::fs::read_to_string(f).ok() })
                    .await;
                let ok = this.update_in(cx, |this, window, cx| {
                    if let Some(disk) = disk {
                        this.on_disk(disk, window, cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        });
        let focus = state.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        Self {
            path,
            file,
            state,
            saved: text,
            conflict: None,
            theme,
            _poll: poll,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }

    fn buffer(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    pub fn dirty(&self, cx: &App) -> bool {
        self.buffer(cx) != self.saved
    }

    /// Linha e coluna do cursor, para a barra de status.
    pub fn cursor(&self, cx: &App) -> (usize, usize) {
        let s = self.state.read(cx);
        let text = s.value();
        let o = s.cursor().min(text.len());
        let before = &text[..o];
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count());
        (before.matches('\n').count() + 1, col + 1)
    }

    pub fn selection_ref(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        line_ref(&self.path, &s.value(), s.selected_range())
    }

    fn on_disk(&mut self, disk: String, window: &mut Window, cx: &mut Context<Self>) {
        let buffer = self.buffer(cx);
        match sync(&self.saved, &buffer, &disk) {
            Sync::Same => {
                if disk == buffer {
                    self.saved = disk;
                    self.conflict = None;
                }
                return;
            }
            Sync::Reload => self.load(disk, window, cx),
            Sync::Conflict => {
                if self.conflict.as_ref() == Some(&disk) {
                    return;
                }
                self.conflict = Some(disk);
            }
        }
        cx.emit(Event::Changed);
        cx.notify();
    }

    fn load(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let at = s.cursor().min(text.len());
            s.set_value(text.clone(), window, cx);
            s.set_selected_range(at..at, cx);
        });
        self.saved = text;
        self.conflict = None;
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer(cx);
        if text == self.saved {
            return cx.emit(Event::Toast("Nada a salvar".into()));
        }
        match std::fs::write(&self.file, &text) {
            Ok(()) => {
                self.saved = text;
                self.conflict = None;
                cx.emit(Event::Toast(format!("Salvo: {}", self.path)));
            }
            Err(e) => cx.emit(Event::Toast(format!(
                "Não deu para salvar {}: {e}",
                self.path
            ))),
        }
        cx.emit(Event::Changed);
        cx.notify();
    }
}

impl Render for FileEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.key == "s" && k.modifiers.control && !k.modifiers.shift {
                    this.save(cx);
                    cx.stop_propagation();
                }
            }))
            .when(self.conflict.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .items_center()
                        .px(px(12.))
                        .py(px(6.))
                        .flex_none()
                        .bg(Hsla {
                            a: 0.12,
                            ..t.orange
                        })
                        .border_b_1()
                        .border_color(t.orange)
                        .child(
                            div()
                                .text_color(t.orange)
                                .child("Claude alterou este arquivo enquanto você editava"),
                        )
                        .child(div().flex_1())
                        .child(
                            button(&t, "cf-diff", "Ver diff", false).on_click(cx.listener(
                                |this, _, _, cx| cx.emit(Event::ShowDiff(this.path.clone())),
                            )),
                        )
                        .child(
                            button(&t, "cf-mine", "Manter meu", false).on_click(cx.listener(
                                |this, _, window, cx| {
                                    // o próximo Ctrl+S grava o seu por cima
                                    if let Some(disk) = this.conflict.take() {
                                        this.saved = disk;
                                    }
                                    window.focus(&this.focus_handle(cx), cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(button(&t, "cf-theirs", "Aceitar dele", true).on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(disk) = this.conflict.take() {
                                    this.load(disk, window, cx);
                                }
                                window.focus(&this.focus_handle(cx), cx);
                                cx.emit(Event::Changed);
                                cx.notify();
                            }),
                        )),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.state)
                        .appearance(false)
                        .bordered(false)
                        .h_full()
                        .font_family(t.mono.clone())
                        .text_size(px(13.))
                        .line_height(px(20.)),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Sync, line_ref, sync};

    #[test]
    fn disco_mudou() {
        assert_eq!(sync("a", "a", "a"), Sync::Same);
        assert_eq!(sync("a", "a", "b"), Sync::Reload);
        assert_eq!(sync("a", "meu", "b"), Sync::Conflict);
        assert_eq!(sync("a", "b", "b"), Sync::Same);
    }

    #[test]
    fn referencia_da_selecao() {
        let t = "um\ndois\ntres\n";
        assert_eq!(line_ref("x.ts", t, 4..4), "@x.ts:2");
        assert_eq!(line_ref("x.ts", t, 0..9), "@x.ts:1-3");
    }
}
